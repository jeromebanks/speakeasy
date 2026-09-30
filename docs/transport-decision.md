# Transport decision: Iroh + iroh-blobs trial (Milestone 0)

Status: **adopted for the Milestone 1 prototype.** Local and same-host
results are below. Transfers between two machines on different networks
have **not yet been verified** (see [Owner-run verification](#owner-run-two-machine-verification)).

## Decision

Use `iroh` for peer connections and `iroh-blobs` for opaque artifact transfer
in the first implementation slice. The Speakeasy publication contract
(publisher identity, signed manifest, artifact hashes, verified local export)
stays independent of Iroh: artifact identifiers are plain BLAKE3 hashes of raw
bytes, and signed metadata is Speakeasy's own encoding with its own publisher
key (see [manifest-format.md](manifest-format.md)).

The trial found no blocker, so no alternative stack (Hypercore, libp2p, Willow)
was evaluated.

## Tested versions and sources

| Item | Value |
| --- | --- |
| `iroh` | `=1.3.0` (crates.io, released 2026-09-28), MIT OR Apache-2.0 |
| `iroh-blobs` | `=0.103.0` (crates.io, released 2026-06-15; depends on `iroh ^1.0.0`), MIT OR Apache-2.0 |
| Rust | 1.96.1 stable; iroh and iroh-blobs declare MSRV 1.91 |
| Host | macOS 15 (Darwin 24.6.0), Apple Silicon (arm64) |
| Sources | crates.io API metadata; the crates' own README, examples and source in the Cargo registry |

Both crates are dual-licensed MIT OR Apache-2.0, which leaves Speakeasy's own
license choice open. Speakeasy's license is still undecided (`publish = false`
in `Cargo.toml`). A full dependency-license audit (for example with
`cargo deny`) has not been run.

## Maturity notes

- The iroh-blobs 0.103 README says: *"this version of iroh-blobs is not yet
  considered production quality. For now, if you need production quality, use
  iroh-blobs 0.35."* Speakeasy is itself a prototype, so the trial proceeds.
  Speakeasy uses only the fetch-by-hash, `FsStore`, tags and provider APIs, so
  that a later upgrade or replacement stays small.
- iroh itself is 1.x (1.0.0 released 2026-06-15; minor releases roughly every
  2–3 weeks since).
- API facts observed while building the spike:
  - `Endpoint::online()` waits for a home-relay connection. With
    `RelayMode::Disabled`, or with no internet access, **it never returns**.
    Iroh's docs say this. Offline/LAN code paths must not call it.
  - `Router::shutdown()` already shuts down the blob store through
    `BlobsProtocol`'s shutdown hook. Calling `FsStore::shutdown()` afterwards
    fails with "Sender closed".
  - `FsStore` has garbage collection **off by default** (`Options::gc = None`).
    Speakeasy still sets persistent tags on installed artifacts, so enabling GC
    later cannot delete served data.

## Networking behavior and external dependencies

`presets::N0` configures:

- **n0 relays** (the spike observed `https://usw1-1.relay.n0.iroh.link./`).
  Relays are used for connection setup and as a fallback path. They are
  free shared infrastructure run by number0, not an owner-operated service.
- **pkarr/DNS address lookup** (`PkarrPublisher::n0_dns`, `PkarrResolver`,
  `DnsAddressLookup`). The endpoint **publishes its reachable addresses,
  including its public IP, to n0's public DNS service**, keyed by endpoint
  id. Anyone who knows an endpoint id can look up that node's IP addresses.
  This does not discover feeds, but it is an external and public dependency.
  Speakeasy therefore makes relay and address-lookup usage configurable.
  Setting both off gives a LAN/explicit-address mode with no n0 infrastructure.
- Tickets (`BlobTicket`) embed the full `EndpointAddr`: endpoint id, relay URL,
  and LAN and public IP addresses. **A ticket reveals the serving peer's IP
  addresses to whoever receives it.**

`presets::Minimal` with `RelayMode::Disabled` uses no n0 infrastructure. Peers
must then be given explicit IP addresses.

## Privacy and access control of this prototype

- Anyone who can reach a serving peer can request any complete blob in its
  store **by hash**. There is no per-feed authorization.
- Transport encryption (QUIC/TLS between endpoint keys) protects bytes in
  flight from network observers only. It does not restrict which peers may
  fetch.
- This is a **public-data prototype**. Do not use it for confidential community
  information until Milestone 2 (encrypted private publications) exists and is
  tested.

## Observed results

### Local, in-process (loopback, no relay, no address lookup)

Command: `cargo run --example iroh_spike -- local <empty-dir>`

Three nodes (A, B, C) run in one process, each with its own `FsStore`,
using `presets::Minimal`, `RelayMode::Disabled`, bound to `127.0.0.1:0`, with
full `EndpointAddr`s exchanged in process. Fixture: 24 MiB of deterministic
BLAKE3-XOF bytes.

| Check | Result |
| --- | --- |
| iroh-blobs raw `Hash` equals plain `blake3::hash(bytes)` | yes |
| A→B aborted after first progress event (16 384 bytes reported) | aborted |
| B's store closed and reopened from disk; partial data retained | 540 672 bytes local, incomplete |
| Resumed fetch transferred only the missing bytes | 24 625 152 = 25 165 824 − 540 672 |
| B export re-hashed independently | matches |
| Repeat fetch after completion | 0 payload bytes |
| A shut down; B dials A | timed out (5 s) |
| C fetched from B with A offline | 25 165 824 bytes, ~0.46 s |
| C export re-hashed independently | matches |

### Same host, separate processes, n0 preset (internet required)

Commands: `iroh_spike serve <rootA> <8 MiB file>`, then `iroh_spike fetch
<rootB> <ticket> <out>`. Stop A, then `serve <rootB>`, then fetch into C.
Both processes ran on one Mac on a home LAN (public IP redacted here).

| Variant | Connect | Transfer 8 MiB | Active path |
| --- | --- | --- | --- |
| Full ticket | 6 ms | 0.19 s | IP (LAN address) |
| `SPIKE_RELAY_ONLY=1` (IP transports cleared) | 119 ms | 5.8 s (~11.6 Mbit/s) | n0 relay |
| `SPIKE_ID_ONLY=1` (dial by id, pkarr/DNS lookup) | 293 ms | 0.54 s | IP (LAN), relay also active |
| Reseed: A stopped, C fetched from B | 9 ms | 0.25 s | IP (LAN) |

All exports matched the source bytes (`cmp` and BLAKE3).

**This is not evidence that two machines on different networks can connect.**
It shows only that this host can reach n0 relays and pkarr/DNS, and that
the relay path carries data.

### Not yet tested

- Two machines on different networks (NAT traversal, direct vs relayed paths
  across the internet).
- A malicious provider sending corrupted bytes. BLAKE3 verified streaming is
  expected to reject them, but only Speakeasy's post-export re-hash was
  exercised. Milestone 1 tests corruption at the export/install layer.
- Long-running seeding, many concurrent peers, large (GB-scale) artifacts.

## Owner-run two-machine verification

These commands need two machines on different networks, for example the
Mac mini at home and a laptop on a phone hotspot. Record the printed `path`
lines.

```sh
# On both machines
git clone https://github.com/jeromebanks/speakeasy && cd speakeasy
cargo build --release --example iroh_spike
S=./target/release/examples/iroh_spike

# Machine A (Mac mini)
head -c 67108864 /dev/urandom > /tmp/spike-payload.bin
$S serve /tmp/spike-a /tmp/spike-payload.bin      # copy the printed ticket

# Machine B (different network)
$S fetch /tmp/spike-b '<ticket>' /tmp/spike-out.bin
SPIKE_RELAY_ONLY=1 $S fetch /tmp/spike-b-relay '<ticket>' /tmp/spike-out-relay.bin

# Interrupted transfer: Ctrl-C the first fetch midway, rerun the same command,
# and confirm payload_bytes_read < 67108864 and local_bytes_before > 0.

# Reseed: stop A (Ctrl-C), then on B:
$S serve /tmp/spike-b                              # copy the ticket
# Machine C (or A with a fresh root) fetches from B:
$S fetch /tmp/spike-c '<ticket-from-B>' /tmp/spike-out-c.bin
```

Until someone runs these commands and records the results here, the
internet-connectivity criteria of Milestone 0 remain **unverified**.

## Milestone 1 plan

The Milestone 1 plan is implemented in this repository as the `speakeasy`
crate. See [manifest-format.md](manifest-format.md) and the README.

1. The publisher's Ed25519 key is separate from the iroh endpoint key. Both are
   persisted under the runtime root with mode 0600.
2. The signed manifest (feed, sequence, time, schema id, artifact paths,
   BLAKE3 hashes and sizes) uses an explicit length-prefixed encoding with a
   domain-separation tag, and has golden-vector tests.
3. A small custom ALPN (`speakeasy/head/1`) returns the latest signed manifest
   a peer holds for a feed. The client verifies it against the publisher key
   pinned in its descriptor, never against the serving peer.
4. Artifacts are fetched per hash with `Remote::fetch`, which skips data
   already held locally. The unit of reuse is a whole artifact; within one
   artifact, only interrupted ranges resume.
5. Installation happens on disk: stage, re-hash, fsync, rename into
   `versions/<seq>`, then atomically swap the `current` pointer. Rollback
   protection uses the highest accepted sequence number.
6. Serving uses the blob store, which holds persistent tags for installed
   versions. The data sits on disk twice: once in the blob store and once in
   the export directory.
