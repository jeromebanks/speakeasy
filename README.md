# Speakeasy

**Publish locally. Share by invitation.**

Speakeasy is a proposed local-first, peer-to-peer content publishing system. Publishers produce versioned artifacts; subscribers fetch and verify them, keep local copies, and can help seed them to other peers. Applications consume those copies without requiring a central service or running AI themselves.

The immediate workload is distributing periodically compiled venue/event datasets to multiple independent consumers. Speakeasy treats payloads as opaque bytes and supports a local publication metadata catalog. Payload schemas, curation, semantic validation, and content lookup indexes belong to producers and consumers. Other knowledge and analytical artifacts can use the same boundary.

**Status: prototype.** Milestone 0 (Iroh trial) and the smallest authenticated publish → replicate → reseed path from Milestone 1 are implemented in Rust. Local multi-process tests pass. **Connectivity between two machines on different networks has not been verified yet.** This is a **public-data prototype**: no confidentiality or access control. See [docs/transport-decision.md](docs/transport-decision.md), [docs/manifest-format.md](docs/manifest-format.md) and [docs/known-issues.md](docs/known-issues.md).

Start with [INIT.md](INIT.md). Agent contributors must follow [AGENTS.md](AGENTS.md); [CLAUDE.md](CLAUDE.md) imports those instructions for Claude Code.

Enterprise evolution should preserve stable identities, versioned formats, configurable deployment settings, and boundaries for later policy and audit integration. The MVP adds no enterprise control plane or mandatory cloud service.

## Brand

Speakeasy evokes communities that share useful information through personal connections. The proposed visual direction is a small illuminated doorway and a discreet invitation card: warm amber, charcoal, simple typography. Do not use blockchain or anonymity claims in the branding. The working name was chosen by the project owner; trademark, package, and domain availability have not been established.

## Usage

Requires Rust ≥ 1.91 (tested with 1.96.1) on macOS/Apple Silicon. Linux is expected to work but is untested.

```sh
cargo build --release
SE=./target/release/speakeasy          # every command prints JSON on stdout

# Publisher (A). Runtime roots default to ~/.speakeasy; never put them in a git checkout.
$SE --root ~/sp-a init --publisher
$SE --root ~/sp-a publish --feed sample-events --from ./export-dir --schema example/1 --attr coverage=fictional
$SE --root ~/sp-a descriptor sample-events > sample-events.descriptor.json   # share out of band
$SE --root ~/sp-a serve                                   # prints {"endpoint_id": …, "ticket": "endpoint…"}

# Subscriber (B)
$SE --root ~/sp-b init
$SE --root ~/sp-b subscribe sample-events.descriptor.json --peer <A-endpoint-id>   # stable in n0 mode; tickets go stale
$SE --root ~/sp-b sync sample-events                      # verify + install atomically
$SE --root ~/sp-b export sample-events                    # {"current_path": ".../current", "version_path": ...}
$SE --root ~/sp-b status sample-events                    # installed version, last sync, errors
$SE --root ~/sp-b serve --sync-every 300                  # reseed to others, keep syncing

# C can sync from B with A offline; C still verifies A's signature.
```

Other commands: `list`, `inspect <feed> [--sequence N]`. Global options:
- `--network n0|local`. `n0` (the default) uses n0 relays and pkarr/DNS lookup and **publishes this node's IP addresses**. `local` uses neither and needs tickets with reachable IP addresses.
- `--bind ADDR`.

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | ok (including "up to date") |
| 1 | error |
| 2 | usage |
| 3 | verification/rejection (bad signature, wrong publisher, rollback, equivocation) |
| 4 | unavailable (no peer reachable, or no peer has the feed) |

Known limits:
- One process per root. Stop `serve` before `publish`; a subscriber that serves uses `serve --sync-every`.
- Each version is stored twice: blob store plus export.
- No freshness guarantee: a peer can withhold newer versions.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                                      # unit, CLI multi-process (loopback), crash-recovery tests
cargo run --example iroh_spike -- local "$(mktemp -d)/spike"   # Milestone 0 transport spike
```

A default debug build of the dependency tree needs about 3 GB. On a machine short of disk space, prefix the commands with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0` (about 0.7 GB).

The tests use temporary roots and synthetic fixtures, and run on loopback in `--network local` mode. They are **not** evidence of connectivity across networks. For the owner-run two-machine procedure, see [docs/transport-decision.md](docs/transport-decision.md#owner-run-two-machine-verification). With the CLI, run the Usage steps above on two machines in `n0` mode and record the results in that document.

## First milestone

Publish a synthetic dataset on one machine, synchronize it to another over a real peer connection, then demonstrate that the second machine can serve it to a third while the original publisher is offline. Updates must be verified and installed atomically; readers must work offline.

Private publications and cryptographic invitations are the next stage. The public-data prototype must never be presented as suitable for confidential community information.

GitHub stores code, documentation, and synthetic fixtures. It is not the publication store for real events or community membership.
