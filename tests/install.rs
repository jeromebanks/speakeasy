//! Library-level tests: crash recovery at each install step, and an
//! interrupted network transfer resumed by `sync`.

use std::{fs, path::Path, time::Duration};

use iroh_blobs::Hash;
use n0_future::StreamExt;
use speakeasy::{
    manifest::{Artifact, Limits, Manifest, SignedManifest},
    net::{NetConfig, NetworkMode, Node, open_store},
    ops::{self, PublishOptions, SyncOutcome},
    repo::{self, FeedDir, Repo},
};

fn publish_opts(feed: &str, from: &Path) -> PublishOptions {
    PublishOptions {
        feed: feed.into(),
        from: from.into(),
        content_type: "application/octet-stream".into(),
        schema: String::new(),
        description: String::new(),
        attributes: vec![],
    }
}

/// Import `files` into the store and sign version `seq` with the repo's key.
async fn signed_version(
    repo: &Repo,
    store: &iroh_blobs::api::Store,
    feed: &str,
    seq: u64,
    files: &[(&str, &[u8])],
) -> (SignedManifest, Vec<iroh_blobs::api::TempTag>) {
    let key = repo.publisher_key().unwrap().unwrap();
    let mut tags = Vec::new();
    let mut artifacts = Vec::new();
    for (path, data) in files {
        let tt = store.add_slice(data).temp_tag().await.unwrap();
        artifacts.push(Artifact {
            path: path.to_string(),
            size: data.len() as u64,
            hash: *tt.hash().as_bytes(),
        });
        tags.push(tt);
    }
    let m = Manifest {
        publisher: key.verifying_key().to_bytes(),
        feed: feed.into(),
        sequence: seq,
        published_at: 1,
        content_type: String::new(),
        schema: String::new(),
        description: String::new(),
        attributes: vec![],
        artifacts,
    };
    (m.sign(&key, &Limits::default()).unwrap(), tags)
}

fn read(feed: &FeedDir, rel: &str) -> Vec<u8> {
    fs::read(feed.current_link().join(rel)).unwrap()
}

#[tokio::test]
async fn crash_at_each_install_step_recovers_to_a_complete_version() {
    let tmp = tempfile::tempdir().unwrap();
    let limits = Limits::default();
    let repo = Repo::init(&tmp.path().join("root"), true).unwrap();
    let input = tmp.path().join("v1");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("data.txt"), b"version one").unwrap();
    let (store, _lock) = open_store(&repo).await.unwrap();
    ops::publish(&repo, &store, publish_opts("f", &input), &limits)
        .await
        .unwrap();
    let feed = repo.resolve_feed("f").unwrap();
    assert_eq!(read(&feed, "data.txt"), b"version one");

    let v2: &[(&str, &[u8])] = &[("data.txt", b"version two")];

    // Crash after staging only.
    let (signed, _t) = signed_version(&repo, &store, "f", 2, v2).await;
    repo::tag_artifacts(&store, feed.id, &signed).await.unwrap();
    repo::stage(&store, &feed, &signed).await.unwrap();
    feed.recover(&limits).unwrap();
    assert_eq!(feed.current_sequence().unwrap(), Some(1));
    assert_eq!(feed.state().unwrap().installed.unwrap().sequence, 1);
    assert_eq!(fs::read_dir(feed.dir.join("staging")).unwrap().count(), 0);
    assert_eq!(read(&feed, "data.txt"), b"version one");

    // Crash after placing versions/2 but before the pointer swap.
    let staging = repo::stage(&store, &feed, &signed).await.unwrap();
    repo::place_version(&feed, &signed, &staging).unwrap();
    assert!(feed.version_dir(2).exists());
    feed.recover(&limits).unwrap();
    assert!(!feed.version_dir(2).exists());
    assert_eq!(feed.state().unwrap().installed.unwrap().sequence, 1);
    assert_eq!(read(&feed, "data.txt"), b"version one");

    // Crash after the pointer swap but before state.json is updated.
    let staging = repo::stage(&store, &feed, &signed).await.unwrap();
    repo::place_version(&feed, &signed, &staging).unwrap();
    repo::swap_current(&feed, 2).unwrap();
    assert_eq!(feed.state().unwrap().installed.unwrap().sequence, 1);
    feed.recover(&limits).unwrap();
    let installed = feed.state().unwrap().installed.unwrap();
    assert_eq!(installed.sequence, 2);
    assert_eq!(installed.manifest_hash, hex::encode(signed.manifest_hash()));
    assert_eq!(read(&feed, "data.txt"), b"version two");

    // The next publish continues from the recovered sequence.
    fs::write(input.join("data.txt"), b"version three").unwrap();
    let r = ops::publish(&repo, &store, publish_opts("f", &input), &limits)
        .await
        .unwrap();
    assert_eq!(r.sequence, 3);
    // Retention: current and previous version kept, older pruned.
    assert!(!feed.version_dir(1).exists());
    assert!(feed.version_dir(2).exists() && feed.version_dir(3).exists());
    assert!(!feed.manifest_path(1).exists());
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn staging_rejects_bytes_that_do_not_match_the_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let limits = Limits::default();
    let repo = Repo::init(&tmp.path().join("root"), true).unwrap();
    let input = tmp.path().join("v1");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("a.txt"), b"one").unwrap();
    let (store, _lock) = open_store(&repo).await.unwrap();
    ops::publish(&repo, &store, publish_opts("f", &input), &limits)
        .await
        .unwrap();
    let feed = repo.resolve_feed("f").unwrap();
    // A manifest whose declared size disagrees with the stored bytes.
    let (mut signed, _t) = signed_version(&repo, &store, "f", 2, &[("a.txt", b"two")]).await;
    signed.manifest.artifacts[0].size = 4;
    assert!(repo::install(&store, &feed, &signed).await.is_err());
    feed.recover(&limits).unwrap();
    assert_eq!(feed.state().unwrap().installed.unwrap().sequence, 1);
    assert_eq!(read(&feed, "a.txt"), b"one");
    // Missing artifact (never imported) is rejected before anything is placed.
    let mut missing = signed.clone();
    missing.manifest.artifacts[0] = Artifact {
        path: "a.txt".into(),
        size: 5,
        hash: *Hash::new(b"never").as_bytes(),
    };
    assert!(repo::stage(&store, &feed, &missing).await.is_err());
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupted_transfer_resumes_on_next_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let limits = Limits::default();
    let net = NetConfig {
        mode: NetworkMode::Local,
        bind: Some("127.0.0.1:0".parse().unwrap()),
    };
    let a_repo = Repo::init(&tmp.path().join("a"), true).unwrap();
    let b_repo = Repo::init(&tmp.path().join("b"), false).unwrap();
    let input = tmp.path().join("input");
    fs::create_dir_all(&input).unwrap();
    let mut big = vec![0u8; 16 * 1024 * 1024];
    blake3::Hasher::new()
        .update(b"interrupt")
        .finalize_xof()
        .fill(&mut big);
    fs::write(input.join("big.bin"), &big).unwrap();
    {
        let (store, _lock) = open_store(&a_repo).await.unwrap();
        ops::publish(&a_repo, &store, publish_opts("f", &input), &limits)
            .await
            .unwrap();
        store.shutdown().await.unwrap();
    }
    let a_feed = a_repo.resolve_feed("f").unwrap();
    let desc = ops::descriptor(&a_feed, &[]).unwrap();
    let b_feed = ops::subscribe(&b_repo, &desc, &[]).unwrap();

    let a = Node::open(a_repo, &net).await.unwrap();
    let ticket = a.ticket().await.to_string();
    let hash = Hash::new(&big);

    // First attempt: abort the raw transfer once at least 1 MiB has arrived.
    // (Aborting after only the first 16 KiB left nothing persisted in trials.)
    let b = Node::open(b_repo.clone(), &net).await.unwrap();
    {
        let conn = b
            .endpoint()
            .connect(
                speakeasy::net::parse_peer(&ticket).unwrap(),
                iroh_blobs::ALPN,
            )
            .await
            .unwrap();
        let mut stream = b.store.remote().fetch(conn, hash).stream();
        while let Some(item) = stream.next().await {
            if let iroh_blobs::api::remote::GetProgressItem::Progress(n) = item
                && n >= 1024 * 1024
            {
                break;
            }
        }
    }
    b.shutdown().await.unwrap();

    // B restarts; sync completes, transferring only the missing remainder.
    let b = Node::open(b_repo, &net).await.unwrap();
    let local = b.store.remote().local(hash).await.unwrap();
    assert!(
        !local.is_complete() && local.local_bytes() > 0,
        "complete={} local_bytes={}",
        local.is_complete(),
        local.local_bytes()
    );
    let report = ops::sync_feed(&b, &b_feed, &[ticket], &limits, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(report.outcome, SyncOutcome::Updated);
    assert_eq!(
        report.bytes_transferred + local.local_bytes(),
        big.len() as u64
    );
    assert_eq!(
        fs::read(b_feed.current_link().join("big.bin")).unwrap(),
        big
    );
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}
