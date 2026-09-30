//! End-to-end tests driving the `speakeasy` binary as separate processes on
//! loopback (`--network local`: no relays, no address lookup). These are
//! local-process tests, not evidence of cross-network connectivity.

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_speakeasy");

fn se(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("--root")
        .arg(root)
        .args(["--network", "local", "--bind", "127.0.0.1:0"])
        .args(args)
        .output()
        .expect("run speakeasy")
}

fn ok(root: &Path, args: &[&str]) -> Value {
    let out = se(root, args);
    assert!(
        out.status.success(),
        "speakeasy {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("json stdout")
}

fn fails(root: &Path, args: &[&str], code: i32) -> Value {
    let out = se(root, args);
    assert_eq!(
        out.status.code(),
        Some(code),
        "speakeasy {args:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stderr).expect("json stderr")
}

struct Server {
    child: Child,
    ticket: String,
}

impl Server {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(BIN)
            .arg("--root")
            .arg(root)
            .args(["--network", "local", "--bind", "127.0.0.1:0", "serve"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn serve");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let v: Value = serde_json::from_str(&line).expect("serve banner");
        Self {
            child,
            ticket: v["ticket"].as_str().unwrap().to_string(),
        }
    }

    fn stop(mut self) {
        self.terminate();
    }

    fn terminate(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            self.terminate();
        }
    }
}

/// Deterministic synthetic bytes.
fn synthetic(seed: &str, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    blake3::Hasher::new()
        .update(seed.as_bytes())
        .finalize_xof()
        .fill(&mut out);
    out
}

fn write_fixture(dir: &Path, events_seed: &str) {
    fs::create_dir_all(dir.join("events")).unwrap();
    fs::write(
        dir.join("venues.csv"),
        "id,name\n1,The Imaginary Lounge\n2,Nowhere Hall\n",
    )
    .unwrap();
    fs::write(
        dir.join("events/2026-10.bin"),
        synthetic(events_seed, 100_000),
    )
    .unwrap();
    fs::write(dir.join("README.txt"), "Fictional fixture data.\n").unwrap();
}

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn current(root: &Path, feed: &str) -> PathBuf {
    let v = ok(root, &["export", feed]);
    PathBuf::from(v["current_path"].as_str().unwrap())
}

fn installed_seq(root: &Path, feed: &str) -> Option<u64> {
    ok(root, &["status", feed])["installed"]["sequence"].as_u64()
}

#[test]
fn publish_replicate_reseed_and_reject() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let [a, b, c, e, d] = ["a", "b", "c", "e", "d"].map(|n| t.join(n));
    let feed = "sample-events";

    // A publishes v1.
    let init = ok(&a, &["init", "--publisher"]);
    let publisher = init["publisher"].as_str().unwrap().to_string();
    let v1 = t.join("fixture-v1");
    write_fixture(&v1, "events v1");
    let p1 = ok(
        &a,
        &[
            "publish",
            "--feed",
            feed,
            "--from",
            v1.to_str().unwrap(),
            "--schema",
            "example.events/1",
            "--attr",
            "coverage=fictional",
        ],
    );
    assert_eq!(p1["sequence"], 1);
    let desc_path = t.join("descriptor.json");
    fs::write(
        &desc_path,
        serde_json::to_vec(&ok(&a, &["descriptor", feed])).unwrap(),
    )
    .unwrap();

    for r in [&b, &c, &e, &d] {
        ok(r, &["init"]);
        ok(r, &["subscribe", desc_path.to_str().unwrap()]);
    }

    // B and E sync v1 from A.
    let server_a = Server::start(&a);
    let s = ok(&b, &["sync", feed, "--peer", &server_a.ticket]);
    assert_eq!(s["outcome"], "updated");
    assert_eq!(s["sequence"], 1);
    assert_eq!(s["bytes_transferred"].as_u64(), p1["total_size"].as_u64());
    assert_eq!(tree(&current(&b, feed)), tree(&v1));
    // Idempotent repeat.
    let s = ok(&b, &["sync", feed, "--peer", &server_a.ticket]);
    assert_eq!(s["outcome"], "up_to_date");
    assert_eq!(s["bytes_transferred"], 0);
    ok(&e, &["sync", feed, "--peer", &server_a.ticket]);
    server_a.stop();

    // A publishes v2 with one changed artifact; B fetches only that artifact.
    let v2 = t.join("fixture-v2");
    write_fixture(&v2, "events v2");
    let p2 = ok(
        &a,
        &[
            "publish",
            "--feed",
            feed,
            "--from",
            v2.to_str().unwrap(),
            "--schema",
            "example.events/1",
        ],
    );
    assert_eq!(p2["sequence"], 2);
    let server_a = Server::start(&a);
    let s = ok(&b, &["sync", feed, "--peer", &server_a.ticket]);
    assert_eq!(s["sequence"], 2);
    assert_eq!(s["artifacts_fetched"], 1);
    assert_eq!(s["bytes_transferred"], 100_000);
    assert_eq!(tree(&current(&b, feed)), tree(&v2));
    let a_ticket = server_a.ticket.clone();
    server_a.stop();

    // A offline: sync fails as unavailable, previous version and last success kept.
    let before = ok(&b, &["status", feed]);
    let err = fails(
        &b,
        &["sync", feed, "--peer", &a_ticket, "--timeout", "5"],
        4,
    );
    assert_eq!(err["kind"], "unavailable");
    let after = ok(&b, &["status", feed]);
    assert_eq!(after["installed"], before["installed"]);
    assert_eq!(
        after["last_successful_sync"],
        before["last_successful_sync"]
    );
    assert!(after["last_error"].is_string());

    // B reseeds to C with A offline; C verifies A's signature.
    let server_b = Server::start(&b);
    let s = ok(&c, &["sync", feed, "--peer", &server_b.ticket]);
    assert_eq!(s["sequence"], 2);
    assert_eq!(tree(&current(&c, feed)), tree(&v2));
    let m = ok(&c, &["inspect", feed]);
    assert_eq!(m["publisher"], publisher.as_str());
    assert_eq!(m["schema"], "example.events/1");
    assert_eq!(m["artifacts"].as_array().unwrap().len(), 3);

    // Rollback: E still serves v1; C (at v2) rejects it and keeps v2.
    let server_e = Server::start(&e);
    let err = fails(&c, &["sync", feed, "--peer", &server_e.ticket], 3);
    assert!(err["error"].as_str().unwrap().contains("rollback"), "{err}");
    assert_eq!(installed_seq(&c, feed), Some(2));
    // Falls through to a good peer after a bad one.
    let s = ok(
        &c,
        &[
            "sync",
            feed,
            "--peer",
            &server_e.ticket,
            "--peer",
            &server_b.ticket,
        ],
    );
    assert_eq!(s["outcome"], "up_to_date");
    server_e.stop();
    server_b.stop();

    // Offline reads with every server stopped.
    assert_eq!(tree(&current(&c, feed)), tree(&v2));
    assert_eq!(installed_seq(&c, feed), Some(2));

    // Tampered manifest served by B: D (fresh) and C both reject it.
    let feed_id = p1["feed_id"].as_str().unwrap();
    let mpath = b.join("feeds").join(feed_id).join("manifests/2.manifest");
    let original = fs::read(&mpath).unwrap();
    let mut bad = original.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0x01;
    fs::write(&mpath, &bad).unwrap();
    let server_b = Server::start(&b);
    let err = fails(&d, &["sync", feed, "--peer", &server_b.ticket], 3);
    assert!(
        err["error"].as_str().unwrap().contains("signature"),
        "{err}"
    );
    assert_eq!(installed_seq(&d, feed), None);
    fails(&c, &["sync", feed, "--peer", &server_b.ticket], 3);
    assert_eq!(installed_seq(&c, feed), Some(2));
    server_b.stop();
    fs::write(&mpath, &original).unwrap();
}

#[test]
fn corrupted_blob_is_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let [a, b] = ["a", "b"].map(|n| t.join(n));
    ok(&a, &["init", "--publisher"]);
    let v1 = t.join("fixture");
    write_fixture(&v1, "corruption");
    ok(
        &a,
        &["publish", "--feed", "f", "--from", v1.to_str().unwrap()],
    );
    let desc = t.join("d.json");
    fs::write(
        &desc,
        serde_json::to_vec(&ok(&a, &["descriptor", "f"])).unwrap(),
    )
    .unwrap();
    ok(&b, &["init"]);
    ok(&b, &["subscribe", desc.to_str().unwrap()]);

    // Flip bytes in the large artifact's data file inside A's blob store.
    let hash = blake3::hash(&synthetic("corruption", 100_000))
        .to_hex()
        .to_string();
    let data_file = find_file(&a.join("blobs"), &hash).expect("blob data file");
    let mut bytes = fs::read(&data_file).unwrap();
    assert_eq!(bytes.len(), 100_000);
    bytes[50_000] ^= 0xff;
    fs::write(&data_file, &bytes).unwrap();

    let server = Server::start(&a);
    let out = se(
        &b,
        &["sync", "f", "--peer", &server.ticket, "--timeout", "20"],
    );
    assert!(!out.status.success(), "corrupted content was accepted");
    let err = String::from_utf8_lossy(&out.stderr);
    eprintln!("corrupted fetch error: {err}");
    assert!(err.contains("fetch events/2026-10.bin"), "{err}");
    assert_eq!(installed_seq(&b, "f"), None);
    server.stop();
}

fn find_file(dir: &Path, needle: &str) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, needle) {
                return Some(f);
            }
        } else if p
            .file_name()
            .is_some_and(|n| n.to_string_lossy() == format!("{needle}.data"))
        {
            return Some(p);
        }
    }
    None
}

#[test]
fn descriptor_and_input_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let a = t.join("a");
    ok(&a, &["init", "--publisher"]);

    // Symlinks in publish input are rejected, not followed.
    let input = t.join("input");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("ok.txt"), "x").unwrap();
    std::os::unix::fs::symlink("/etc/hosts", input.join("link")).unwrap();
    let err = fails(
        &a,
        &["publish", "--feed", "f", "--from", input.to_str().unwrap()],
        1,
    );
    assert!(err["error"].as_str().unwrap().contains("symlink"));
    fs::remove_file(input.join("link")).unwrap();

    // Case-insensitive collisions are rejected.
    fs::write(input.join("OK.txt"), "y").unwrap();
    let names: Vec<_> = fs::read_dir(&input).unwrap().collect();
    if names.len() == 2 {
        // Case-sensitive filesystem: both files exist, so publish must reject.
        fails(
            &a,
            &["publish", "--feed", "f", "--from", input.to_str().unwrap()],
            1,
        );
        fs::remove_file(input.join("OK.txt")).unwrap();
    }

    // Invalid feed names are rejected.
    fails(
        &a,
        &[
            "publish",
            "--feed",
            "Bad Name",
            "--from",
            input.to_str().unwrap(),
        ],
        1,
    );

    ok(
        &a,
        &["publish", "--feed", "f", "--from", input.to_str().unwrap()],
    );
    let mut desc = ok(&a, &["descriptor", "f"]);

    // A descriptor whose feed_id does not match its publisher/feed is rejected.
    let b = t.join("b");
    ok(&b, &["init"]);
    desc["feed_id"] = Value::String("00".repeat(32));
    let path = t.join("bad.json");
    fs::write(&path, serde_json::to_vec(&desc).unwrap()).unwrap();
    fails(&b, &["subscribe", path.to_str().unwrap()], 1);

    // A descriptor pinning a different publisher never matches A's feed.
    let other = t.join("other");
    let other_pub = ok(&other, &["init", "--publisher"])["publisher"]
        .as_str()
        .unwrap()
        .to_string();
    desc["publisher"] = Value::String(other_pub.clone());
    desc["feed_id"] = Value::String(
        speakeasy::manifest::FeedId::derive(
            &hex::decode(&other_pub).unwrap().try_into().unwrap(),
            "f",
        )
        .to_hex(),
    );
    fs::write(&path, serde_json::to_vec(&desc).unwrap()).unwrap();
    ok(&b, &["subscribe", path.to_str().unwrap()]);
    let server = Server::start(&a);
    let err = fails(&b, &["sync", "f", "--peer", &server.ticket], 4);
    assert!(
        err["error"].as_str().unwrap().contains("does not hold"),
        "{err}"
    );
    server.stop();

    // Concurrent store use fails fast.
    let server = Server::start(&a);
    let err = fails(
        &a,
        &["publish", "--feed", "f", "--from", input.to_str().unwrap()],
        1,
    );
    assert!(err["error"].as_str().unwrap().contains("in use"));
    server.stop();
}
