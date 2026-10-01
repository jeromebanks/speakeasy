# Handoff: 2026-09-30

Branch `m0-iroh-spike` (pushed). Reviewed commit: `aab3728`. Read
[AGENTS.md](../AGENTS.md), [INIT.md](../INIT.md) and this file before
continuing.

## Milestone status

| Milestone | Status |
| --- | --- |
| **0: Iroh trial** | **Done locally and on one host; internet test pending.** iroh 1.3.0 and iroh-blobs 0.103.0 were adopted ([transport-decision.md](transport-decision.md)). Verified so far:<br>• A→B transfer, then B→C with A offline<br>• interrupted transfer resumed<br>• BLAKE3 re-check of received bytes<br>• direct, relay-only and id-only dialing, all on one Mac<br>**Not verified:** two machines on different networks (NAT traversal, relay vs direct). |
| **1: Verified publication and replication** | **Smallest path done locally; internet test pending; hardening open.** All 7 minimum behaviors from INIT.md are implemented and covered by local multi-process tests (table below). Open issues #1–#8 cover limits, durability and recovery hygiene. Publishing requires stopping `serve`. |
| After the first exchange (locality), 2 (private), later | Not started. Do not start until the two-machine verification is recorded. |

### Milestone 1 criteria → evidence

| # | Criterion (INIT.md) | Evidence | How far it's proven |
| --- | --- | --- | --- |
| 1 | Persistent publisher identity signs v1 | `manifest::tests::*`, golden vector; `tests/cli.rs` | local |
| 2 | Descriptor pins the publisher; hash alone is not trusted | `descriptor_and_input_validation`; spec in [manifest-format.md](manifest-format.md) | local |
| 3 | Peer transfer, verify, complete local version; exact signed bytes | `publish_replicate_reseed_and_reject`; manifest spec | local + same-host n0 |
| 4 | v2 fetches only missing artifacts | CLI test (exactly 100 000 bytes); n0 run (500 000 bytes) | local + same-host n0 |
| 5 | Interrupted, invalid, corrupt or missing data keeps the previous version; atomic install; restart recovery | `corrupted_or_missing_update_preserves_previous_version`, `interrupted_transfer_resumes_on_next_sync`, `crash_at_each_install_step_…`, tampered-manifest case | local (simulated crashes; see #8) |
| 6 | A offline; B reads locally and serves C; C verifies A's signature | CLI test; n0 run | local + same-host n0 |
| 7 | Idempotent sync; status shows version, last success, freshness; failure ≠ success | CLI tests (up_to_date with 0 bytes, failed sync keeps last success) | local |

Also covered: rollback rejection, equivocation rejection, path-safety and
symlink-race rejection, limits on the signed manifest, and limits and
deadlines on the head protocol.

## Is an external client test needed?

**Yes.** It is the only remaining acceptance step for Milestones 0 and 1. All
evidence so far comes from one Mac. Local-process and same-host runs
don't exercise NAT traversal, so the internet-connectivity criteria stay
*unverified* until a real second machine on another network has been tested.

### Using a Chromebook as the second machine

A Chromebook works if its **Linux development environment** (Crostini) is
turned on. It also adds the first Linux evidence (Linux is untested so far).

1. ChromeOS Settings → About ChromeOS → Developers → Linux development
   environment → Turn on. Allow at least 10 GB of disk.
2. In the Linux terminal:
   ```sh
   sudo apt-get update && sudo apt-get install -y build-essential git curl
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   . ~/.cargo/env
   git clone https://github.com/jeromebanks/speakeasy && cd speakeasy
   git checkout m0-iroh-spike
   cargo build --release          # may take 10–30 min on a Chromebook
   ```
3. Put the Chromebook on a **different network** from the Mac, for example
   a phone hotspot. If it shares the home Wi-Fi, the test only shows the LAN
   path.
4. Run the CLI procedure in
   [transport-decision.md → Owner-run two-machine verification](transport-decision.md#owner-run-two-machine-verification).
   - Mac = A, publisher and seed.
   - Chromebook = B: syncs from A, then serves after A stops.
   - C = a third root on either machine. A fresh root on the Mac
     (`--root /tmp/sp-c`) is fine; C must then sync from B with A's
     `serve` stopped.

   Use **bare endpoint ids** as peers (n0 mode). Copy the descriptor JSON
   between machines by any means; it contains no secrets.
5. Record the following for each step in `transport-decision.md`:
   - connect time and transfer time
   - `bytes_transferred`
   - the path used: relay or direct

   The spike's `fetch` prints the active path, so for that detail also run
   `cargo run --release --example iroh_spike -- fetch …` against the Mac's
   `iroh_spike serve`.

Expected Crostini behavior: the Linux container sits behind ChromeOS NAT and
then the hotspot's NAT. Outbound dialing should work. Inbound (B serving C)
may fall back to the n0 relay. Both outcomes are valid evidence; record which
one happened.

If the Chromebook can't run Linux apps, any other machine works (Linux
VPS, a friend's laptop, a second Mac on a hotspot).

## Next steps, in order

1. **Two-machine verification** (above). Update `transport-decision.md` and
   mark the Milestone 0 and 1 network criteria verified or failed, with output.
   If it fails, document the NAT/relay behavior before changing anything.
2. **Hardening, in issue order:**
   - #1: enforce declared sizes during fetch.
   - #6: take the lock for metadata writes.
   - #5: cancellable sync loop.
   - #3: build read-only views from `current`.
   - #4: reconcile tags and manifests during recovery.
   - #2: fsync parent directories.
   - #7: exit codes.
   - #8: real crash tests.
3. **Publish while serving:** let `publish` reach a running `serve` instead of
   requiring it to stop. The likely approach is iroh-blobs' `rpc` feature or a
   small local control socket. Keep it minimal.
4. Merge `m0-iroh-spike` to `main` after steps 1–2 (open a PR; the owner
   reviews).
5. Only then: locality preferences, or Milestone 2 (private publications).
   The Milestone 2 boundary design is in INIT.md.

## Owner decisions pending

- License (crate is `publish = false`).
- `CLAUDE.md` still says "documentation only"; it's out of date.
- Whether to move `~/.cargo/registry` to the external drive.

## Environment notes for the next session

- **Build output and test temp files** go to the external drive through a
  local, untracked `.cargo/config.toml`:
  - target: `/Volumes/YOTUO/dev-cache/speakeasy/target`
  - temp: `…/tmp`

  The internal disk was full earlier. Put scratch data there too.
- **Verification:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
  The last full run passed: 11 unit, 5 CLI and 4 library tests.
- **Tests and CLI modes:** tests use `--network local` on loopback. The
  default `--network n0` uses n0 relays and publishes node IP addresses through
  pkarr/DNS.
- **Prototype limits:** this is a public-data prototype. Never use real
  event data or community information.
