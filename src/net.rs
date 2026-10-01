//! Iroh networking: endpoint setup, the blob store, and the small
//! `speakeasy/head/1` protocol that returns the latest signed manifest a peer
//! holds for a feed. Transport identity (node key) is not publisher identity.

use std::{net::SocketAddr, str::FromStr, sync::Arc, time::Duration};

use tokio::sync::Semaphore;

use anyhow::{Context, Result, anyhow, bail, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{BlobsProtocol, store::fs::FsStore};
use iroh_tickets::endpoint::EndpointTicket;

use crate::{manifest::FeedId, repo::Repo};

pub const HEAD_ALPN: &[u8] = b"speakeasy/head/1";
/// Deadline for a head exchange: stream accept, request read, response write.
pub const HEAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Time to let the client read the response and close before we drop it.
const HEAD_LINGER: Duration = Duration::from_secs(2);
/// Concurrent head exchanges served; excess connections are closed at once.
pub const MAX_CONCURRENT_HEAD: usize = 64;
const HEAD_FOUND: u8 = 1;
const HEAD_NOT_FOUND: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum NetworkMode {
    /// n0 public relays and pkarr/DNS address lookup (publishes this node's
    /// addresses, including public IP, keyed by endpoint id).
    N0,
    /// No relays and no address lookup; peers must be given with IP addresses.
    Local,
}

#[derive(Debug, Clone)]
pub struct NetConfig {
    pub mode: NetworkMode,
    pub bind: Option<SocketAddr>,
}

/// Exclusive lock on a runtime root, held while the blob store is open. The
/// store itself blocks (rather than fails) when another process holds it.
pub struct RootLock(#[allow(dead_code)] std::fs::File);

/// Lock the root and open the blob store. Fails fast if another process
/// (e.g. `serve`) holds the root.
pub async fn open_store(repo: &Repo) -> Result<(FsStore, RootLock)> {
    let path = repo.root().join("lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    file.try_lock().map_err(|_| {
        anyhow!(
            "{} is in use by another speakeasy process (e.g. `serve`); stop it first",
            repo.root().display()
        )
    })?;
    let store = FsStore::load(repo.blobs_dir())
        .await
        .map_err(|e| anyhow!("open blob store at {}: {e:#}", repo.blobs_dir().display()))?;
    Ok((store, RootLock(file)))
}

pub struct Node {
    pub repo: Repo,
    pub store: FsStore,
    pub router: Router,
    mode: NetworkMode,
    _lock: RootLock,
}

impl Node {
    pub async fn open(repo: Repo, net: &NetConfig) -> Result<Self> {
        let (store, lock) = open_store(&repo).await?;
        let secret = SecretKey::from_bytes(&repo.node_secret()?);
        let mut builder = match net.mode {
            NetworkMode::N0 => Endpoint::builder(presets::N0),
            NetworkMode::Local => {
                Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
            }
        }
        .secret_key(secret);
        if let Some(bind) = net.bind {
            builder = builder.bind_addr(bind)?;
        }
        let endpoint = builder.bind().await?;
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
            .accept(
                HEAD_ALPN,
                HeadProtocol {
                    repo: repo.clone(),
                    permits: Arc::new(Semaphore::new(MAX_CONCURRENT_HEAD)),
                },
            )
            .spawn();
        Ok(Self {
            repo,
            store,
            router,
            mode: net.mode,
            _lock: lock,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    /// This node's current address as a ticket. In n0 mode, waits briefly for
    /// a home relay; `Endpoint::online` never returns without one.
    pub async fn ticket(&self) -> EndpointTicket {
        if self.mode == NetworkMode::N0 {
            let _ = tokio::time::timeout(Duration::from_secs(10), self.endpoint().online()).await;
        }
        EndpointTicket::new(self.endpoint().addr())
    }

    pub async fn shutdown(self) -> Result<()> {
        // BlobsProtocol's shutdown hook also shuts down the store.
        self.router.shutdown().await?;
        Ok(())
    }
}

/// Parse a peer given as an endpoint ticket or a bare endpoint id (the latter
/// needs address lookup, i.e. n0 mode).
pub fn parse_peer(s: &str) -> Result<EndpointAddr> {
    if let Ok(t) = EndpointTicket::from_str(s) {
        return Ok(t.endpoint_addr().clone());
    }
    if let Ok(id) = EndpointId::from_str(s) {
        return Ok(EndpointAddr::from(id));
    }
    bail!("peer must be an endpoint ticket or endpoint id: {s:?}")
}

#[derive(Debug, Clone)]
struct HeadProtocol {
    repo: Repo,
    permits: Arc<Semaphore>,
}

impl HeadProtocol {
    fn lookup(&self, feed: [u8; 32]) -> Option<Vec<u8>> {
        let feed = self.repo.feed(FeedId(feed));
        feed.installed_manifest_bytes().ok().flatten()
    }

    async fn respond(&self, connection: &Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let request = recv.read_to_end(32).await.map_err(AcceptError::from_err)?;
        let Ok(feed) = <[u8; 32]>::try_from(request) else {
            connection.close(1u32.into(), b"bad request");
            return Ok(());
        };
        match self.lookup(feed) {
            Some(bytes) => {
                send.write_all(&[HEAD_FOUND])
                    .await
                    .map_err(AcceptError::from_err)?;
                send.write_all(&bytes)
                    .await
                    .map_err(AcceptError::from_err)?;
            }
            None => {
                send.write_all(&[HEAD_NOT_FOUND])
                    .await
                    .map_err(AcceptError::from_err)?;
            }
        }
        send.finish()?;
        Ok(())
    }
}

impl ProtocolHandler for HeadProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // Bound concurrency and lifetime: a peer that connects and never
        // sends (or never reads) holds a permit for at most the deadline.
        let Ok(_permit) = self.permits.clone().try_acquire_owned() else {
            connection.close(2u32.into(), b"busy");
            return Ok(());
        };
        match tokio::time::timeout(HEAD_REQUEST_TIMEOUT, self.respond(&connection)).await {
            Ok(res) => res?,
            Err(_) => {
                connection.close(3u32.into(), b"timeout");
                return Ok(());
            }
        }
        let _ = tokio::time::timeout(HEAD_LINGER, connection.closed()).await;
        Ok(())
    }
}

/// Ask a peer for its latest signed manifest of `feed`. Returns unverified
/// bytes (bounded by `max_bytes`); the caller must verify them.
pub async fn fetch_head(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    feed: FeedId,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>> {
    let conn = endpoint
        .connect(peer, HEAD_ALPN)
        .await
        .context("connect for head")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&feed.0).await?;
    send.finish()?;
    let resp = recv
        .read_to_end(1 + max_bytes)
        .await
        .context("read head response")?;
    conn.close(0u32.into(), b"done");
    ensure!(!resp.is_empty(), "empty head response");
    match resp[0] {
        HEAD_FOUND => Ok(Some(resp[1..].to_vec())),
        HEAD_NOT_FOUND => Ok(None),
        other => bail!("unknown head status {other}"),
    }
}
