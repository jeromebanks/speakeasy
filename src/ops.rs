//! Command implementations shared by the CLI and tests.

use std::{collections::HashSet, fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result, ensure};
use ed25519_dalek::VerifyingKey;
use iroh_blobs::{Hash, store::fs::FsStore};
use serde::{Deserialize, Serialize};

use crate::{
    input::{InputDir, file_stream},
    manifest::{Artifact, FeedId, Limits, Manifest, SignedManifest, validate_feed_name},
    net::{Node, fetch_head, parse_peer},
    repo::{self, FeedDir, Installed, Repo, Role, Subscription, now_unix},
};

pub const DESCRIPTOR_FORMAT: &str = "speakeasy-descriptor/1";

/// Failure classes that map to distinct CLI exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Signature, publisher, rollback, equivocation or content verification failed.
    Verification,
    /// No peer could be reached or none had the feed.
    Unavailable,
    /// A local install/storage step failed. Sync stops instead of trying
    /// other peers, because local state may be mid-commit.
    Local,
}

#[derive(Debug)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

fn verification(msg: impl Into<String>) -> anyhow::Error {
    Failure {
        kind: FailureKind::Verification,
        message: msg.into(),
    }
    .into()
}

fn unavailable(msg: impl Into<String>) -> anyhow::Error {
    Failure {
        kind: FailureKind::Unavailable,
        message: msg.into(),
    }
    .into()
}

pub fn failure_kind(err: &anyhow::Error) -> Option<FailureKind> {
    err.chain()
        .find_map(|e| e.downcast_ref::<Failure>())
        .map(|f| f.kind)
}

// ---------------------------------------------------------------- publish

pub struct PublishOptions {
    pub feed: String,
    pub from: PathBuf,
    pub content_type: String,
    pub schema: String,
    pub description: String,
    pub attributes: Vec<(String, String)>,
}

#[derive(Debug, Serialize)]
pub struct PublishReport {
    pub feed_id: String,
    pub feed: String,
    pub sequence: u64,
    pub manifest_hash: String,
    pub artifacts: usize,
    pub total_size: u64,
    pub version_path: PathBuf,
}

pub async fn publish(
    repo: &Repo,
    store: &FsStore,
    opts: PublishOptions,
    limits: &Limits,
) -> Result<PublishReport> {
    let key = repo
        .publisher_key()?
        .context("no publisher key in this root; run `speakeasy init --publisher`")?;
    let publisher = key.verifying_key().to_bytes();
    validate_feed_name(&opts.feed)?;
    // Refuse input that overlaps the runtime root (keys, store, feeds).
    let from_real = fs::canonicalize(&opts.from)
        .with_context(|| format!("publish input {}", opts.from.display()))?;
    let root_real = fs::canonicalize(repo.root())?;
    ensure!(
        !from_real.starts_with(&root_real) && !root_real.starts_with(&from_real),
        "publish input must not overlap the speakeasy root"
    );
    let input = InputDir::open(&opts.from)?;
    let files = input.list()?;
    crate::paths::validate_path_set(files.iter().map(String::as_str))?;
    ensure!(
        files.len() <= limits.max_artifacts,
        "too many files ({})",
        files.len()
    );

    let feed = repo.add_feed(Subscription {
        format: repo::REPO_FORMAT,
        publisher: hex::encode(publisher),
        feed: opts.feed.clone(),
        role: Role::Publisher,
        peers: vec![],
    })?;
    feed.recover(limits)?;
    let sequence = feed.state()?.installed.map(|i| i.sequence + 1).unwrap_or(1);

    // Temp tags keep imported blobs alive until install sets persistent tags.
    let mut temp_tags = Vec::new();
    let mut artifacts = Vec::new();
    let mut total: u64 = 0;
    for path in files {
        let (file, size) = input.open_file(&path)?;
        ensure!(
            size <= limits.max_artifact_size,
            "{path} exceeds artifact size limit"
        );
        total = total.saturating_add(size);
        ensure!(
            total <= limits.max_total_size,
            "input exceeds total size limit"
        );
        let tag = store
            .blobs()
            .add_stream(file_stream(file, size))
            .await
            .temp_tag()
            .await
            .with_context(|| format!("import {path}"))?;
        artifacts.push(Artifact {
            path,
            size,
            hash: *tag.hash().as_bytes(),
        });
        temp_tags.push(tag);
    }

    let mut attributes = opts.attributes;
    attributes.sort();
    let manifest = Manifest {
        publisher,
        feed: opts.feed.clone(),
        sequence,
        published_at: now_unix(),
        content_type: opts.content_type,
        schema: opts.schema,
        description: opts.description,
        attributes,
        artifacts,
    };
    let signed = manifest.sign(&key, limits)?;
    // Staging re-hashes each exported file, so a file modified during import
    // fails here instead of being published with a mismatched size.
    let version_path = repo::install(store, &feed, &signed).await?;
    drop(temp_tags);
    Ok(PublishReport {
        feed_id: feed.id.to_hex(),
        feed: opts.feed,
        sequence,
        manifest_hash: hex::encode(signed.manifest_hash()),
        artifacts: signed.manifest.artifacts.len(),
        total_size: signed.manifest.total_size(),
        version_path,
    })
}

// ---------------------------------------------------------- descriptors

/// Shareable subscription descriptor. `publisher` + `feed` are the trust
/// anchor; `peers` are unsigned hints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Descriptor {
    pub format: String,
    pub publisher: String,
    pub feed: String,
    pub feed_id: String,
    #[serde(default)]
    pub peers: Vec<String>,
}

pub fn descriptor(feed: &FeedDir, extra_peers: &[String]) -> Result<Descriptor> {
    let sub = feed.subscription()?;
    let mut peers = sub.peers.clone();
    for p in extra_peers {
        parse_peer(p)?;
        if !peers.contains(p) {
            peers.push(p.clone());
        }
    }
    Ok(Descriptor {
        format: DESCRIPTOR_FORMAT.into(),
        publisher: sub.publisher,
        feed: sub.feed,
        feed_id: feed.id.to_hex(),
        peers,
    })
}

pub fn subscribe(repo: &Repo, desc: &Descriptor, extra_peers: &[String]) -> Result<FeedDir> {
    ensure!(
        desc.format == DESCRIPTOR_FORMAT,
        "unsupported descriptor format {:?}",
        desc.format
    );
    validate_feed_name(&desc.feed)?;
    let publisher: [u8; 32] = hex::decode(&desc.publisher)
        .ok()
        .and_then(|b| b.try_into().ok())
        .context("descriptor publisher must be 32 bytes of hex")?;
    VerifyingKey::from_bytes(&publisher).context("descriptor publisher is not a valid key")?;
    let id = FeedId::derive(&publisher, &desc.feed);
    ensure!(
        id.to_hex() == desc.feed_id,
        "descriptor feed_id does not match publisher and feed name"
    );
    let mut peers = desc.peers.clone();
    peers.extend(extra_peers.iter().cloned());
    for p in &peers {
        parse_peer(p)?;
    }
    if let Ok(existing) = repo.feed(id).subscription() {
        ensure!(
            existing.role == Role::Subscriber,
            "this root publishes that feed"
        );
    }
    repo.add_feed(Subscription {
        format: repo::REPO_FORMAT,
        publisher: desc.publisher.clone(),
        feed: desc.feed.clone(),
        role: Role::Subscriber,
        peers,
    })
}

// ----------------------------------------------------------------- sync

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutcome {
    Updated,
    UpToDate,
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub feed_id: String,
    pub feed: String,
    pub outcome: SyncOutcome,
    pub previous_sequence: Option<u64>,
    pub sequence: u64,
    pub peer: String,
    pub artifacts_fetched: usize,
    pub bytes_transferred: u64,
    pub version_path: PathBuf,
}

pub async fn sync_feed(
    node: &Node,
    feed: &FeedDir,
    peers_override: &[String],
    limits: &Limits,
    peer_timeout: Duration,
) -> Result<SyncReport> {
    let sub = feed.subscription()?;
    ensure!(
        sub.role == Role::Subscriber,
        "this root is the publisher of {}; nothing to sync",
        sub.feed
    );
    feed.recover(limits)?;
    let peers = if peers_override.is_empty() {
        sub.peers.clone()
    } else {
        peers_override.to_vec()
    };
    let mut state = feed.state()?;
    state.last_attempt = Some(now_unix());
    feed.write_state(&state)?;

    let mut errors = Vec::new();
    let mut kind = FailureKind::Unavailable;
    for peer in &peers {
        let res =
            tokio::time::timeout(peer_timeout, sync_from_peer(node, feed, &sub, peer, limits))
                .await
                .unwrap_or_else(|_| Err(unavailable(format!("timed out after {peer_timeout:?}"))));
        match res {
            Ok(report) => {
                let mut state = feed.state()?;
                state.last_successful_sync = Some(now_unix());
                state.last_sync_peer = Some(peer.clone());
                state.last_error = None;
                feed.write_state(&state)?;
                return Ok(report);
            }
            Err(e) => {
                match failure_kind(&e) {
                    Some(FailureKind::Verification) => kind = FailureKind::Verification,
                    Some(FailureKind::Local) => {
                        // Do not fall through to other peers: their heads would
                        // be compared against possibly stale state. Reconcile
                        // with the committed pointer and stop.
                        let message = format!("{}: {e:#}", short_peer(peer));
                        let recovered = feed.recover(limits);
                        let mut state = feed.state()?;
                        state.last_error = Some(message.clone());
                        feed.write_state(&state)?;
                        recovered?;
                        return Err(Failure {
                            kind: FailureKind::Local,
                            message,
                        }
                        .into());
                    }
                    _ => {}
                }
                errors.push(format!("{}: {e:#}", short_peer(peer)));
            }
        }
    }
    let message = if peers.is_empty() {
        "no peers configured; pass --peer or subscribe with peers".to_string()
    } else {
        errors.join("; ")
    };
    let mut state = feed.state()?;
    state.last_error = Some(message.clone());
    feed.write_state(&state)?;
    Err(Failure { kind, message }.into())
}

fn short_peer(p: &str) -> String {
    match parse_peer(p) {
        Ok(addr) => addr.id.fmt_short().to_string(),
        Err(_) => p.chars().take(16).collect(),
    }
}

async fn sync_from_peer(
    node: &Node,
    feed: &FeedDir,
    sub: &Subscription,
    peer: &str,
    limits: &Limits,
) -> Result<SyncReport> {
    let addr = parse_peer(peer)?;
    let bytes = fetch_head(
        node.endpoint(),
        addr.clone(),
        feed.id,
        limits.max_manifest_bytes,
    )
    .await
    .map_err(|e| unavailable(format!("{e:#}")))?
    .ok_or_else(|| unavailable("peer does not hold this feed"))?;
    let signed = SignedManifest::verify(&bytes, &sub.publisher_bytes()?, limits)
        .map_err(|e| verification(format!("manifest rejected: {e:#}")))?;
    if signed.manifest.feed != sub.feed {
        return Err(verification("manifest is for a different feed"));
    }
    let seq = signed.manifest.sequence;
    let hash_hex = hex::encode(signed.manifest_hash());
    let state = feed.state()?;
    let previous = state.installed.clone();
    if let Some(Installed {
        sequence,
        manifest_hash,
        ..
    }) = &previous
    {
        if seq < *sequence {
            return Err(verification(format!(
                "rollback rejected: peer offered sequence {seq}, installed is {sequence}"
            )));
        }
        if seq == *sequence {
            if &hash_hex != manifest_hash {
                return Err(verification(format!(
                    "equivocation: two different signed manifests for sequence {seq}"
                )));
            }
            return Ok(SyncReport {
                feed_id: feed.id.to_hex(),
                feed: sub.feed.clone(),
                outcome: SyncOutcome::UpToDate,
                previous_sequence: Some(*sequence),
                sequence: seq,
                peer: peer.to_string(),
                artifacts_fetched: 0,
                bytes_transferred: 0,
                version_path: feed.version_dir(seq),
            });
        }
    }

    // Fetch artifacts missing locally. Unit of reuse: whole artifacts by hash;
    // partially received artifacts resume from the ranges already stored.
    let conn = node
        .endpoint()
        .connect(addr, iroh_blobs::ALPN)
        .await
        .map_err(|e| unavailable(format!("connect for blobs: {e:#}")))?;
    let mut seen = HashSet::new();
    let mut bytes_transferred = 0;
    let mut artifacts_fetched = 0;
    for a in &signed.manifest.artifacts {
        let hash = Hash::from_bytes(a.hash);
        if !seen.insert(hash) || node.store.remote().local(hash).await?.is_complete() {
            continue;
        }
        let stats = node
            .store
            .remote()
            .fetch(conn.clone(), hash)
            .await
            .map_err(|e| unavailable(format!("fetch {}: {e:#}", a.path)))?;
        bytes_transferred += stats.payload_bytes_read;
        artifacts_fetched += 1;
    }
    conn.close(0u32.into(), b"done");
    let version_path = repo::install(&node.store, feed, &signed)
        .await
        .map_err(|e| {
            anyhow::Error::from(Failure {
                kind: FailureKind::Local,
                message: format!("install failed: {e:#}"),
            })
        })?;
    Ok(SyncReport {
        feed_id: feed.id.to_hex(),
        feed: sub.feed.clone(),
        outcome: SyncOutcome::Updated,
        previous_sequence: previous.map(|i| i.sequence),
        sequence: seq,
        peer: peer.to_string(),
        artifacts_fetched,
        bytes_transferred,
        version_path,
    })
}

// --------------------------------------------------------- status/inspect

#[derive(Debug, Serialize)]
pub struct Status {
    pub feed_id: String,
    pub feed: String,
    pub publisher: String,
    pub role: Role,
    pub installed: Option<Installed>,
    /// Seconds since the publisher-claimed publication time of the installed version.
    pub published_age_secs: Option<u64>,
    pub current_path: Option<PathBuf>,
    pub last_successful_sync: Option<u64>,
    pub last_sync_peer: Option<String>,
    pub last_attempt: Option<u64>,
    pub last_error: Option<String>,
    pub peers: Vec<String>,
}

pub fn status(feed: &FeedDir) -> Result<Status> {
    let sub = feed.subscription()?;
    let state = feed.state()?;
    let now = now_unix();
    Ok(Status {
        feed_id: feed.id.to_hex(),
        feed: sub.feed,
        publisher: sub.publisher,
        role: sub.role,
        published_age_secs: state
            .installed
            .as_ref()
            .map(|i| now.saturating_sub(i.published_at)),
        current_path: state.installed.as_ref().map(|_| feed.current_link()),
        installed: state.installed,
        last_successful_sync: state.last_successful_sync,
        last_sync_peer: state.last_sync_peer,
        last_attempt: state.last_attempt,
        last_error: state.last_error,
        peers: sub.peers,
    })
}

#[derive(Debug, Serialize)]
pub struct ArtifactView {
    pub path: String,
    pub size: u64,
    pub blake3: String,
}

#[derive(Debug, Serialize)]
pub struct ManifestView {
    pub feed_id: String,
    pub manifest_hash: String,
    pub publisher: String,
    pub feed: String,
    pub sequence: u64,
    pub published_at: u64,
    pub content_type: String,
    pub schema: String,
    pub description: String,
    pub attributes: std::collections::BTreeMap<String, String>,
    pub total_size: u64,
    pub artifacts: Vec<ArtifactView>,
}

pub fn inspect(feed: &FeedDir, sequence: Option<u64>, limits: &Limits) -> Result<ManifestView> {
    let seq = match sequence {
        Some(s) => s,
        None => {
            feed.state()?
                .installed
                .context("no version installed")?
                .sequence
        }
    };
    let signed = feed.read_manifest(seq, limits)?;
    let m = &signed.manifest;
    Ok(ManifestView {
        feed_id: feed.id.to_hex(),
        manifest_hash: hex::encode(signed.manifest_hash()),
        publisher: hex::encode(m.publisher),
        feed: m.feed.clone(),
        sequence: m.sequence,
        published_at: m.published_at,
        content_type: m.content_type.clone(),
        schema: m.schema.clone(),
        description: m.description.clone(),
        attributes: m.attributes.iter().cloned().collect(),
        total_size: m.total_size(),
        artifacts: m
            .artifacts
            .iter()
            .map(|a| ArtifactView {
                path: a.path.clone(),
                size: a.size,
                blake3: hex::encode(a.hash),
            })
            .collect(),
    })
}

#[derive(Debug, Serialize)]
pub struct ExportView {
    pub feed_id: String,
    pub feed: String,
    pub sequence: u64,
    /// Stable path of this exact version (removed after two newer installs).
    pub version_path: PathBuf,
    /// Symlink that always points at the latest installed version.
    pub current_path: PathBuf,
}

pub fn export(feed: &FeedDir) -> Result<ExportView> {
    let sub = feed.subscription()?;
    let installed = feed.state()?.installed.context("no version installed")?;
    Ok(ExportView {
        feed_id: feed.id.to_hex(),
        feed: sub.feed,
        sequence: installed.sequence,
        version_path: feed.version_dir(installed.sequence),
        current_path: feed.current_link(),
    })
}
