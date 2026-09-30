//! Milestone 0 feasibility spike for iroh + iroh-blobs.
//!
//! This is evidence-gathering code, not the Speakeasy publication contract.
//! See docs/transport-decision.md for results and interpretation.
//!
//! Modes:
//!   iroh_spike local <workdir>
//!       Three in-process nodes on loopback, no relay, no address lookup.
//!       A -> B with an interrupted transfer resumed after a store restart,
//!       then A shut down and B -> C. Verifies bytes by independent BLAKE3.
//!   iroh_spike serve <root> [file]
//!       n0 preset (n0 relays + pkarr/DNS lookup). Adds `file` if given and
//!       prints a ticket for every complete blob in the store. Ctrl-C to stop.
//!   iroh_spike fetch <root> <ticket> <out-file>
//!       n0 preset. Fetches the ticketed blob (resuming from local data),
//!       reports bytes transferred and the active path (relay vs IP), exports
//!       and re-hashes the file. Env: SPIKE_RELAY_ONLY=1 disables IP transports;
//!       SPIKE_ID_ONLY=1 dials by endpoint id via pkarr/DNS address lookup.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    Endpoint, EndpointAddr, RelayMode, TransportAddr,
    endpoint::{TransportAddrUsage, presets},
    protocol::Router,
};
use iroh_blobs::{
    BlobFormat, BlobsProtocol, Hash, api::remote::GetProgressItem, store::fs::FsStore,
    ticket::BlobTicket,
};
use n0_future::StreamExt;

const FIXTURE_LEN: usize = 24 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["local", workdir] => local(Path::new(workdir)).await,
        ["serve", root] => serve(Path::new(root), None).await,
        ["serve", root, file] => serve(Path::new(root), Some(Path::new(file))).await,
        ["fetch", root, ticket, out] => fetch(Path::new(root), ticket, Path::new(out)).await,
        _ => {
            eprintln!(
                "usage: iroh_spike local <workdir> | serve <root> [file] | fetch <root> <ticket> <out>"
            );
            std::process::exit(2);
        }
    }
}

/// Deterministic synthetic bytes: BLAKE3 XOF of a fixed seed.
fn fixture(len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    blake3::Hasher::new()
        .update(b"speakeasy spike fixture v1")
        .finalize_xof()
        .fill(&mut out);
    out
}

struct Node {
    store: FsStore,
    router: Router,
}

impl Node {
    async fn open_local(root: &Path) -> Result<Self> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .bind_addr("127.0.0.1:0")?
            .bind()
            .await?;
        Self::with_endpoint(root, endpoint).await
    }

    /// n0 preset. `SPIKE_RELAY_ONLY=1` removes IP transports so traffic must
    /// use the relay.
    async fn open_n0(root: &Path) -> Result<Self> {
        let mut builder = Endpoint::builder(presets::N0);
        if std::env::var_os("SPIKE_RELAY_ONLY").is_some() {
            builder = builder.clear_ip_transports();
        }
        let endpoint = builder.bind().await?;
        Self::with_endpoint(root, endpoint).await
    }

    async fn with_endpoint(root: &Path, endpoint: Endpoint) -> Result<Self> {
        let store = FsStore::load(root.join("blobs")).await?;
        let blobs = BlobsProtocol::new(&store, None);
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, blobs)
            .spawn();
        Ok(Self { store, router })
    }

    fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    /// Current address. Only waits for a home relay when relays are enabled:
    /// `Endpoint::online` never returns with `RelayMode::Disabled`.
    async fn addr(&self, wait_for_relay: bool) -> EndpointAddr {
        if wait_for_relay
            && tokio::time::timeout(Duration::from_secs(15), self.endpoint().online())
                .await
                .is_err()
        {
            eprintln!("warning: no home relay after 15s; advertising direct addresses only");
        }
        self.endpoint().addr()
    }

    async fn shutdown(self) -> Result<()> {
        // BlobsProtocol's shutdown hook also shuts down the store.
        self.router.shutdown().await.context("router shutdown")?;
        Ok(())
    }
}

/// Fetch `hash` from `addr`, taking local data into account. Returns payload
/// bytes actually read from the network.
async fn fetch_from(node: &Node, addr: EndpointAddr, hash: Hash) -> Result<u64> {
    let conn = node.endpoint().connect(addr, iroh_blobs::ALPN).await?;
    let stats = node.store.remote().fetch(conn, hash).await?;
    Ok(stats.payload_bytes_read)
}

async fn export_and_hash(store: &FsStore, hash: Hash, out: &Path) -> Result<blake3::Hash> {
    store
        .blobs()
        .export(hash, std::path::absolute(out)?)
        .await?;
    let bytes = std::fs::read(out)?;
    Ok(blake3::hash(&bytes))
}

async fn local(workdir: &Path) -> Result<()> {
    ensure!(
        !workdir.exists() || std::fs::read_dir(workdir)?.next().is_none(),
        "workdir must be empty or absent: {}",
        workdir.display()
    );
    std::fs::create_dir_all(workdir)?;
    let data = fixture(FIXTURE_LEN);
    let expected = blake3::hash(&data);

    // A: publisher/seed.
    let a = Node::open_local(&workdir.join("a")).await?;
    let tag = a.store.add_slice(&data).await?;
    let hash = tag.hash;
    a.store.tags().create(tag.hash_and_format()).await?;
    println!("fixture bytes={} blake3={expected}", data.len());
    println!(
        "iroh-blobs hash == plain BLAKE3: {}",
        hash.as_bytes() == expected.as_bytes()
    );
    let a_addr = a.addr(false).await;

    // B: first attempt, aborted after the first progress event.
    let b_root = workdir.join("b");
    let b = Node::open_local(&b_root).await?;
    {
        let conn = b
            .endpoint()
            .connect(a_addr.clone(), iroh_blobs::ALPN)
            .await?;
        let mut stream = b.store.remote().fetch(conn, hash).stream();
        let mut aborted_at = None;
        while let Some(item) = stream.next().await {
            match item {
                GetProgressItem::Progress(n) if n > 0 => {
                    aborted_at = Some(n);
                    break;
                }
                GetProgressItem::Progress(_) => {}
                GetProgressItem::Done(_) => {
                    bail!("transfer completed before abort; enlarge fixture")
                }
                GetProgressItem::Error(e) => bail!("unexpected error: {e}"),
            }
        }
        println!("B aborted first fetch after progress={aborted_at:?}");
    }
    // Simulated restart of B: close endpoint and store, reopen from disk.
    b.shutdown().await?;
    let b = Node::open_local(&b_root).await.context("reopen B")?;
    let local = b.store.remote().local(hash).await?;
    println!(
        "B after restart: local_bytes={} complete={}",
        local.local_bytes(),
        local.is_complete()
    );
    ensure!(!local.is_complete(), "abort did not leave a partial blob");

    let t = Instant::now();
    let read = fetch_from(&b, a_addr.clone(), hash).await?;
    println!(
        "B resumed fetch: payload_bytes_read={read} of {} ({:?})",
        data.len(),
        t.elapsed()
    );
    ensure!(
        (read as usize) < data.len(),
        "resume re-downloaded the whole blob"
    );
    b.store.tags().create(tag.hash_and_format()).await?;
    let b_hash = export_and_hash(&b.store, hash, &workdir.join("b-export.bin")).await?;
    ensure!(b_hash == expected, "B export hash mismatch");
    println!("B export verified by independent BLAKE3");

    // Repeat fetch is a no-op.
    let read = fetch_from(&b, a_addr.clone(), hash).await?;
    println!("B repeat fetch: payload_bytes_read={read}");

    // A goes offline.
    a.shutdown().await?;
    let probe = tokio::time::timeout(
        Duration::from_secs(5),
        b.endpoint().connect(a_addr, iroh_blobs::ALPN),
    )
    .await;
    println!(
        "B connect to A after A shutdown: {}",
        match probe {
            Ok(Ok(_)) => "CONNECTED (unexpected)".to_string(),
            Ok(Err(e)) => format!("error ({e})"),
            Err(_) => "timed out".to_string(),
        }
    );

    // C fetches from B only.
    let b_addr = b.addr(false).await;
    let c = Node::open_local(&workdir.join("c")).await?;
    let t = Instant::now();
    let read = fetch_from(&c, b_addr, hash).await?;
    println!(
        "C fetched from B with A offline: payload_bytes_read={read} ({:?})",
        t.elapsed()
    );
    let c_hash = export_and_hash(&c.store, hash, &workdir.join("c-export.bin")).await?;
    ensure!(c_hash == expected, "C export hash mismatch");
    println!("C export verified by independent BLAKE3");

    b.shutdown().await?;
    c.shutdown().await?;
    println!("LOCAL SPIKE OK");
    Ok(())
}

async fn serve(root: &Path, file: Option<&Path>) -> Result<()> {
    let node = Node::open_n0(root).await?;
    if let Some(file) = file {
        let tag = node
            .store
            .blobs()
            .add_path(std::path::absolute(file)?)
            .await?;
        node.store.tags().create(tag.hash_and_format()).await?;
    }
    let addr = node.addr(true).await;
    println!("endpoint id: {}", addr.id);
    for a in addr.addrs.iter() {
        println!("  advertised addr: {a:?}");
    }
    let hashes = node.store.blobs().list().hashes().await?;
    for hash in hashes {
        if node.store.remote().local(hash).await?.is_complete() {
            let ticket = BlobTicket::new(addr.clone(), hash, BlobFormat::Raw);
            println!("ticket {hash}: {ticket}");
        }
    }
    println!("serving; Ctrl-C to stop");
    tokio::signal::ctrl_c().await?;
    node.shutdown().await
}

async fn fetch(root: &Path, ticket: &str, out: &Path) -> Result<()> {
    let ticket: BlobTicket = ticket.parse().context("parse ticket")?;
    let node = Node::open_n0(root).await?;
    let before = node
        .store
        .remote()
        .local(ticket.hash())
        .await?
        .local_bytes();
    let t = Instant::now();
    // `SPIKE_ID_ONLY=1` drops the ticket's addresses and relies on address lookup.
    let target = if std::env::var_os("SPIKE_ID_ONLY").is_some() {
        EndpointAddr::from(ticket.addr().id)
    } else {
        ticket.addr().clone()
    };
    let conn = node.endpoint().connect(target, iroh_blobs::ALPN).await?;
    println!("connected in {:?}", t.elapsed());
    let stats = node.store.remote().fetch(conn, ticket.hash()).await?;
    println!(
        "local_bytes_before={before} payload_bytes_read={} elapsed={:?}",
        stats.payload_bytes_read,
        t.elapsed()
    );
    if let Some(info) = node.endpoint().remote_info(ticket.addr().id).await {
        for a in info.addrs() {
            let kind = match a.addr() {
                TransportAddr::Relay(_) => "relay",
                TransportAddr::Ip(_) => "ip",
                _ => "other",
            };
            let active = matches!(a.usage(), TransportAddrUsage::Active);
            println!("  path {kind} active={active} {:?}", a.addr());
        }
    }
    node.store
        .tags()
        .create(iroh_blobs::HashAndFormat::raw(ticket.hash()))
        .await?;
    let h = export_and_hash(&node.store, ticket.hash(), out).await?;
    println!(
        "exported {} blake3 matches ticket: {}",
        out.display(),
        h.as_bytes() == ticket.hash().as_bytes()
    );
    node.shutdown().await
}
