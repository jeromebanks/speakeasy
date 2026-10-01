//! On-disk repository: keys, blob store, subscriptions, verified versions.
//!
//! ```text
//! <root>/
//!   keys/node.key                 iroh endpoint secret (0600)
//!   keys/publisher.key            Ed25519 publisher secret (0600, publishers only)
//!   blobs/                        iroh-blobs FsStore (exclusive to one process)
//!   feeds/<feed-id>/
//!     subscription.json           pinned publisher key, feed name, peer hints
//!     state.json                  installed version and sync status
//!     manifests/<seq>.manifest    signed manifest envelopes of retained versions
//!     versions/<seq>/...          verified, read-only exported artifacts
//!     current -> versions/<seq>   atomically swapped symlink
//!     staging/                    in-progress installs; cleared on recovery
//! ```
//!
//! Commands that open the blob store hold its exclusive lock, which also
//! serializes repository mutations. Read-only commands read files only.

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::SigningKey;
use iroh_blobs::{Hash, HashAndFormat, api::Store};
use serde::{Deserialize, Serialize};

use crate::{
    keys,
    manifest::{FeedId, Limits, SignedManifest, validate_feed_name},
};

pub const REPO_FORMAT: u32 = 1;
/// Installed versions kept on disk: the current one and its predecessor, so
/// readers holding the previous path are not broken mid-read.
pub const RETAIN_VERSIONS: usize = 2;

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Publisher,
    Subscriber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub format: u32,
    /// Hex Ed25519 public key; the trust anchor for this feed.
    pub publisher: String,
    pub feed: String,
    pub role: Role,
    /// Unsigned, transient hints about peers that may hold the feed.
    #[serde(default)]
    pub peers: Vec<String>,
}

impl Subscription {
    pub fn publisher_bytes(&self) -> Result<[u8; 32]> {
        hex::decode(&self.publisher)
            .ok()
            .and_then(|b| b.try_into().ok())
            .context("publisher key must be 32 bytes of hex")
    }

    pub fn feed_id(&self) -> Result<FeedId> {
        Ok(FeedId::derive(&self.publisher_bytes()?, &self.feed))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Installed {
    pub sequence: u64,
    pub manifest_hash: String,
    /// Publisher claim copied from the manifest.
    pub published_at: u64,
    pub installed_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedState {
    pub format: u32,
    pub installed: Option<Installed>,
    /// Last time a peer was reached and its signed head verified (whether or
    /// not it was newer). Not proof that no newer version exists anywhere.
    pub last_successful_sync: Option<u64>,
    pub last_sync_peer: Option<String>,
    pub last_attempt: Option<u64>,
    pub last_error: Option<String>,
}

impl Repo {
    pub fn init(root: &Path, publisher: bool) -> Result<Self> {
        let root = std::path::absolute(root)?;
        for d in ["keys", "blobs", "feeds"] {
            fs::create_dir_all(root.join(d))?;
        }
        fs::set_permissions(root.join("keys"), fs::Permissions::from_mode(0o700))?;
        let repo = Self { root };
        if !repo.node_key_path().exists() {
            keys::create_secret_file(&repo.node_key_path(), &keys::random_secret()?)?;
        }
        if publisher && !repo.publisher_key_path().exists() {
            keys::create_secret_file(&repo.publisher_key_path(), &keys::random_secret()?)?;
        }
        Ok(repo)
    }

    pub fn open(root: &Path) -> Result<Self> {
        let root = std::path::absolute(root)?;
        let repo = Self { root };
        ensure!(
            repo.node_key_path().exists(),
            "{} is not an initialized speakeasy root (run `speakeasy init`)",
            repo.root.display()
        );
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    fn node_key_path(&self) -> PathBuf {
        self.root.join("keys/node.key")
    }

    fn publisher_key_path(&self) -> PathBuf {
        self.root.join("keys/publisher.key")
    }

    pub fn node_secret(&self) -> Result<[u8; 32]> {
        keys::read_secret_file(&self.node_key_path())
    }

    pub fn publisher_key(&self) -> Result<Option<SigningKey>> {
        let path = self.publisher_key_path();
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(SigningKey::from_bytes(&keys::read_secret_file(
            &path,
        )?)))
    }

    pub fn feed(&self, id: FeedId) -> FeedDir {
        FeedDir {
            id,
            dir: self.root.join("feeds").join(id.to_hex()),
        }
    }

    pub fn feeds(&self) -> Result<Vec<FeedDir>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.root.join("feeds"))? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(bytes) = name
                .to_str()
                .and_then(|s| hex::decode(s).ok())
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
            else {
                continue;
            };
            let feed = self.feed(FeedId(bytes));
            if feed.subscription_path().exists() {
                out.push(feed);
            }
        }
        out.sort_by_key(|f| f.id.to_hex());
        Ok(out)
    }

    /// Resolve a feed by full id, unique id prefix (>= 8 hex chars), or unique name.
    pub fn resolve_feed(&self, query: &str) -> Result<FeedDir> {
        let mut matches = Vec::new();
        for f in self.feeds()? {
            let hex = f.id.to_hex();
            let by_id = query.len() >= 8 && hex.starts_with(&query.to_ascii_lowercase());
            let by_name = f.subscription()?.feed == query;
            if by_id || by_name {
                matches.push(f);
            }
        }
        match matches.len() {
            0 => bail!("no feed matches {query:?}"),
            1 => Ok(matches.pop().unwrap()),
            n => bail!("{n} feeds match {query:?}; use the feed id"),
        }
    }

    /// Record a subscription (or the publisher's own feed). Idempotent for the
    /// same publisher/feed; peers are merged.
    pub fn add_feed(&self, sub: Subscription) -> Result<FeedDir> {
        validate_feed_name(&sub.feed)?;
        let feed = self.feed(sub.feed_id()?);
        fs::create_dir_all(feed.dir.join("manifests"))?;
        fs::create_dir_all(feed.dir.join("versions"))?;
        fs::create_dir_all(feed.dir.join("staging"))?;
        let sub = match feed.subscription().ok() {
            Some(mut existing) => {
                for p in sub.peers {
                    if !existing.peers.contains(&p) {
                        existing.peers.push(p);
                    }
                }
                if sub.role == Role::Publisher {
                    existing.role = Role::Publisher;
                }
                existing
            }
            None => sub,
        };
        write_json_atomic(&feed.subscription_path(), &sub)?;
        if !feed.state_path().exists() {
            write_json_atomic(
                &feed.state_path(),
                &FeedState {
                    format: REPO_FORMAT,
                    ..Default::default()
                },
            )?;
        }
        Ok(feed)
    }
}

#[derive(Debug, Clone)]
pub struct FeedDir {
    pub id: FeedId,
    pub dir: PathBuf,
}

impl FeedDir {
    fn subscription_path(&self) -> PathBuf {
        self.dir.join("subscription.json")
    }
    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }
    pub fn manifest_path(&self, seq: u64) -> PathBuf {
        self.dir.join("manifests").join(format!("{seq}.manifest"))
    }
    pub fn version_dir(&self, seq: u64) -> PathBuf {
        self.dir.join("versions").join(seq.to_string())
    }
    pub fn current_link(&self) -> PathBuf {
        self.dir.join("current")
    }
    fn staging_dir(&self) -> PathBuf {
        self.dir.join("staging")
    }

    pub fn subscription(&self) -> Result<Subscription> {
        read_json(&self.subscription_path())
    }

    pub fn write_subscription(&self, sub: &Subscription) -> Result<()> {
        write_json_atomic(&self.subscription_path(), sub)
    }

    pub fn state(&self) -> Result<FeedState> {
        read_json(&self.state_path())
    }

    pub fn write_state(&self, state: &FeedState) -> Result<()> {
        write_json_atomic(&self.state_path(), state)
    }

    /// Sequence the `current` symlink points at, if any.
    pub fn current_sequence(&self) -> Result<Option<u64>> {
        match fs::read_link(self.current_link()) {
            Ok(target) => {
                let s = target
                    .to_str()
                    .and_then(|s| s.strip_prefix("versions/"))
                    .and_then(|s| s.parse().ok())
                    .context("malformed current link")?;
                Ok(Some(s))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Load and re-verify a stored manifest against the pinned publisher.
    pub fn read_manifest(&self, seq: u64, limits: &Limits) -> Result<SignedManifest> {
        let sub = self.subscription()?;
        let bytes = fs::read(self.manifest_path(seq))?;
        let signed = SignedManifest::verify(&bytes, &sub.publisher_bytes()?, limits)?;
        ensure!(
            signed.manifest.feed == sub.feed && signed.manifest.sequence == seq,
            "stored manifest does not match feed/sequence"
        );
        Ok(signed)
    }

    /// The installed manifest's raw envelope bytes (for serving to peers).
    pub fn installed_manifest_bytes(&self) -> Result<Option<Vec<u8>>> {
        match self.state()?.installed {
            Some(i) => Ok(Some(fs::read(self.manifest_path(i.sequence))?)),
            None => Ok(None),
        }
    }

    /// Crash recovery. Clears staging, drops uncommitted version directories,
    /// and reconciles state.json with the `current` symlink, which is the
    /// commit point of an install.
    pub fn recover(&self, limits: &Limits) -> Result<()> {
        for entry in fs::read_dir(self.staging_dir())? {
            remove_path(&entry?.path())?;
        }
        let _ = fs::remove_file(self.dir.join("current.tmp"));
        let current = self.current_sequence()?;
        let mut state = self.state()?;
        let recorded = state.installed.as_ref().map(|i| i.sequence);
        match (current, recorded) {
            (Some(c), r) if Some(c) != r => {
                ensure!(
                    r.is_none_or(|r| c > r),
                    "current version {c} is older than recorded version {r:?}; refusing to roll back"
                );
                let signed = self.read_manifest(c, limits)?;
                state.installed = Some(Installed {
                    sequence: c,
                    manifest_hash: hex::encode(signed.manifest_hash()),
                    published_at: signed.manifest.published_at,
                    installed_at: now_unix(),
                });
                self.write_state(&state)?;
            }
            (None, Some(r)) => bail!("state records version {r} but `current` is missing"),
            _ => {}
        }
        // Remove version dirs newer than current (placed but never committed).
        for seq in self.version_sequences()? {
            if current.is_none_or(|c| seq > c) {
                remove_path(&self.version_dir(seq))?;
            }
        }
        Ok(())
    }

    fn version_sequences(&self) -> Result<Vec<u64>> {
        let mut out: Vec<u64> = fs::read_dir(self.dir.join("versions"))?
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        out.sort();
        Ok(out)
    }
}

/// Persistent tag protecting one artifact of one installed version from GC.
fn tag_prefix(feed: FeedId, seq: u64) -> String {
    format!("speakeasy/{}/{seq:020}/", feed.to_hex())
}

pub async fn tag_artifacts(store: &Store, feed: FeedId, signed: &SignedManifest) -> Result<()> {
    let prefix = tag_prefix(feed, signed.manifest.sequence);
    for (i, a) in signed.manifest.artifacts.iter().enumerate() {
        store
            .tags()
            .set(
                format!("{prefix}{i:06}"),
                HashAndFormat::raw(Hash::from_bytes(a.hash)),
            )
            .await?;
    }
    Ok(())
}

/// Export every artifact from the store into a fresh staging directory,
/// re-hash each file independently, make it read-only and fsync.
pub async fn stage(store: &Store, feed: &FeedDir, signed: &SignedManifest) -> Result<PathBuf> {
    let m = &signed.manifest;
    let staging = feed.staging_dir().join(format!(
        "{}-{}-{}",
        m.sequence,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&staging)?;
    let mut dirs = vec![staging.clone()];
    for a in &m.artifacts {
        let hash = Hash::from_bytes(a.hash);
        ensure!(
            store.remote().local(hash).await?.is_complete(),
            "artifact {} ({hash}) is not complete in the local store",
            a.path
        );
        let target = staging.join(&a.path); // path validated by manifest verification
        if let Some(parent) = target.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)?;
            let mut p = parent;
            while p != staging {
                dirs.push(p.to_path_buf());
                p = p.parent().expect("inside staging");
            }
        }
        store
            .blobs()
            .export(hash, &target)
            .await
            .with_context(|| format!("export {}", a.path))?;
        let mut f = fs::File::open(&target)?;
        let mut h = blake3::Hasher::new();
        let n = std::io::copy(&mut f, &mut h)?;
        ensure!(
            n == a.size && h.finalize().as_bytes() == &a.hash,
            "exported artifact {} does not match manifest (size {n}, expected {})",
            a.path,
            a.size
        );
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444))?;
        f.sync_all()?;
    }
    dirs.sort();
    dirs.dedup();
    for d in dirs.iter().rev() {
        fsync_dir(d)?;
    }
    Ok(staging)
}

/// Step 1 of commit: persist the manifest and move staging into versions/<seq>.
pub fn place_version(feed: &FeedDir, signed: &SignedManifest, staging: &Path) -> Result<PathBuf> {
    let seq = signed.manifest.sequence;
    ensure_newer_than_committed(feed, seq)?;
    write_bytes_atomic(&feed.manifest_path(seq), &signed.bytes)?;
    let dest = feed.version_dir(seq);
    if dest.exists() {
        remove_path(&dest)?;
    }
    fs::rename(staging, &dest)?;
    fsync_dir(&feed.dir.join("versions"))?;
    Ok(dest)
}

/// Step 2 of commit (the commit point): atomically repoint `current`.
pub fn swap_current(feed: &FeedDir, seq: u64) -> Result<()> {
    let tmp = feed.dir.join("current.tmp");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(format!("versions/{seq}"), &tmp)?;
    fs::rename(&tmp, feed.current_link())?;
    fsync_dir(&feed.dir)
}

/// Step 3 of commit: record the installed version.
pub fn record_installed(feed: &FeedDir, signed: &SignedManifest) -> Result<()> {
    let mut state = feed.state()?;
    state.installed = Some(Installed {
        sequence: signed.manifest.sequence,
        manifest_hash: hex::encode(signed.manifest_hash()),
        published_at: signed.manifest.published_at,
        installed_at: now_unix(),
    });
    feed.write_state(&state)
}

/// Never replace or go below the committed version: its directory is live for
/// readers, its manifest is what peers are served, and its tags protect its blobs.
fn ensure_newer_than_committed(feed: &FeedDir, seq: u64) -> Result<()> {
    if let Some(current) = feed.current_sequence()? {
        ensure!(
            seq > current,
            "refusing to install sequence {seq}: committed version is {current}"
        );
    }
    Ok(())
}

/// Full install: stage, place, swap, record, then prune old versions.
pub async fn install(store: &Store, feed: &FeedDir, signed: &SignedManifest) -> Result<PathBuf> {
    ensure_newer_than_committed(feed, signed.manifest.sequence)?;
    tag_artifacts(store, feed.id, signed).await?;
    let staging = stage(store, feed, signed).await?;
    let dest = place_version(feed, signed, &staging)?;
    swap_current(feed, signed.manifest.sequence)?;
    record_installed(feed, signed)?;
    prune(store, feed).await?;
    Ok(dest)
}

/// Remove versions (directories, manifests, blob tags) beyond the retention
/// window. Never touches the current version or in-progress staging.
pub async fn prune(store: &Store, feed: &FeedDir) -> Result<()> {
    let Some(current) = feed.current_sequence()? else {
        return Ok(());
    };
    let mut keep: Vec<u64> = feed
        .version_sequences()?
        .into_iter()
        .filter(|s| *s <= current)
        .collect();
    let drop_n = keep.len().saturating_sub(RETAIN_VERSIONS);
    let dropped: Vec<u64> = keep.drain(..drop_n).collect();
    for seq in dropped {
        remove_path(&feed.version_dir(seq))?;
        let _ = fs::remove_file(feed.manifest_path(seq));
        store.tags().delete_prefix(tag_prefix(feed.id, seq)).await?;
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_bytes_atomic(path, &bytes)
}

pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    fsync_dir(dir)
}

fn fsync_dir(dir: &Path) -> Result<()> {
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// Remove a file or directory tree (read-only files in writable
/// directories are removable on Unix).
fn remove_path(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if meta.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}
