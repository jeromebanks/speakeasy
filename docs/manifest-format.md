# Publication manifest and trust model (format v1)

Status: implemented in `src/manifest.rs`, `src/repo.rs`, `src/ops.rs`. The
golden vector test in `src/manifest.rs` pins the exact bytes. Any change to
this layout needs a new domain tag and format version.

## Identities

| Identity | Key | Purpose |
| --- | --- | --- |
| Publisher | Ed25519 (`ed25519-dalek` 3.0.0), `keys/publisher.key` | Signs manifests. It is the only authority for a feed. |
| Node | iroh endpoint key (Ed25519), `keys/node.key` | Authenticates transport connections. Any node may serve bytes. |
| Feed | `FeedId = BLAKE3-derive-key("speakeasy 2026-09 feed id v1", publisher_key ‖ feed_name)` | Stable identifier, independent of network addresses. |

Subscribers never trust a serving node's identity for content. A manifest is
accepted only if it is signed by the publisher key pinned in the subscriber's
descriptor.

## Trust bootstrap: descriptors

A descriptor is shared out of band, for example in a message or a file.
`speakeasy descriptor <feed>` produces one:

```json
{"format":"speakeasy-descriptor/1","publisher":"<64 hex>","feed":"sample-events",
 "feed_id":"<64 hex>","peers":["endpoint…"]}
```

- `publisher` and `feed` are the trust anchor. `subscribe` checks that
  `publisher` is a valid Ed25519 key and that `feed_id` matches the derivation.
- `peers` are **unsigned, transient hints** (iroh endpoint tickets or ids).
  Anyone relaying a descriptor could change them. At worst they point the
  subscriber at peers that don't have the feed or serve rejected data. They
  cannot change what is accepted.
- The descriptor holds no secrets. Anyone holding it can fetch this public-data
  prototype's feed from any peer that serves it.

## Exact signed bytes

All integers are big-endian. `str` means a `u32` byte length followed by that
many UTF-8 bytes.

```text
signed_bytes =
  "speakeasy manifest v1\n"          22 bytes, domain-separation tag
  publisher_key                      32 bytes
  feed_name                          str   [a-z0-9._-], 1..=64, starts alnum
  sequence                           u64   >= 1, strictly increasing per feed
  published_at                       u64   Unix seconds (publisher claim)
  content_type                       str
  schema                             str   producer-defined, opaque
  description                        str
  attribute_count                    u32
  { key str, value str } *           sorted strictly by key bytes, key non-empty
  artifact_count                     u32
  { path str, size u64, blake3 [32] } *  sorted strictly by path bytes

envelope = signed_bytes ‖ Ed25519 signature over signed_bytes (64 bytes)
manifest_hash = BLAKE3(envelope)
```

Decoding must consume `signed_bytes` exactly (no trailing bytes). Signature
verification uses `verify_strict`. Artifact hashes are plain BLAKE3 of the raw
bytes, which is identical to the iroh-blobs raw `Hash` (checked by a test). A
different transport could verify artifacts without Iroh.

`attributes` hold producer-defined coverage and provenance metadata. Speakeasy
does not interpret them, the schema id, or the content type.

## Validation (on both sign and accept)

- **Paths:** relative, `/`-separated. Each component is non-empty, not `.` or
  `..`, at most 255 bytes, and limited to ASCII `[A-Za-z0-9._+~@=,-]`. At most
  1024 bytes and depth 32. No case-insensitive duplicates, and no path may be
  used both as a file and as a directory. The ASCII restriction avoids
  case- and Unicode-normalization collisions on APFS; it can be relaxed later.
- **Limits** (`manifest::Limits`, defaults):
  - manifest ≤ 4 MiB
  - ≤ 10 000 artifacts
  - each artifact ≤ 4 GiB
  - total ≤ 16 GiB
  - text fields ≤ 4096 bytes
  - ≤ 64 attributes

  Limits are checked on the signed manifest **before** any artifact download.
- **Publish input:** regular files only. Symlinks and special files are
  rejected, not followed. Non-UTF-8 names are rejected.

## Acceptance rules (subscriber)

Given the installed version `(seq_i, hash_i)` and a verified head `(seq, hash)`
from a peer:

| Condition | Result |
| --- | --- |
| signature or publisher mismatch, malformed, over limits | reject (exit 3), try next peer |
| `seq < seq_i` | reject as rollback (exit 3) |
| `seq == seq_i`, `hash != hash_i` | reject as equivocation (exit 3) |
| `seq == seq_i`, `hash == hash_i` | up to date; counts as a successful sync |
| `seq > seq_i` | fetch missing artifacts, install |

Rollback protection covers anything older than the installed version. Only
the installed version (and its predecessor on disk) is retained.

**Limitations that signatures cannot solve:**
- A signature does not show that a peer is offering the *newest* version.
  A peer can withhold updates (freeze attack), and `last_successful_sync`
  only means some peer's verified head was reached.
- `published_at` is the publisher's claim, not proof of freshness.
  Consumers decide what staleness is acceptable (`status` reports
  `published_age_secs`).
- A hash proves byte integrity. A signature authenticates the publisher.
  Neither proves that the content is factually correct.

## Installation and recovery

1. Fetch each artifact hash that is not complete locally. iroh-blobs verifies
   BLAKE3 streams and resumes partially stored blobs.
2. Set persistent blob tags `speakeasy/<feed-id>/<seq>/<index>`, so a later GC
   cannot delete served data.
3. **Stage:** export each artifact into `staging/<unique>/`, re-hash it
   independently, compare size and hash, chmod 0444, then fsync files and
   directories.
4. **Place:** atomically write `manifests/<seq>.manifest` (temp file, fsync,
   rename, fsync directory), then rename the staging directory to
   `versions/<seq>`.
5. **Commit point:** create the symlink `current.tmp -> versions/<seq>`,
   rename it over `current`, and fsync the feed directory.
6. Record the installed version in `state.json` (atomic write).
7. Prune versions older than the previous one, along with their manifests and
   tags.

Recovery runs before every store-opening command. It clears `staging/`,
deletes `versions/N` newer than `current` (placed but not committed), and
moves `state.json` forward if `current` is ahead of it. It refuses to move
`current` backwards. Tests cover a crash after each of steps 3, 4 and 5.

Consumers should read through `current/…`, or pin `versions/<seq>/…` from
`speakeasy export`. A pinned path stays valid until two newer versions have
been installed.

## Key storage and backup

- Keys are hex text files with mode 0600 in a 0700 `keys/` directory. Loading
  refuses group- or world-readable key files. Keys are not encrypted at rest.
- **Losing `publisher.key` means losing the ability to publish the feed.**
  There is no rotation or recovery yet. Back it up offline. A new key is a
  new feed identity, and subscribers must re-subscribe.
- `node.key` can be regenerated, which only changes the node's endpoint id.
- Never commit runtime roots or keys. The default root is `~/.speakeasy`.

## Security properties of this prototype

| Property | Status |
| --- | --- |
| Byte integrity of artifacts | independent re-hash before install: implemented and tested. In-transit BLAKE3 verification: provided by iroh-blobs' design. Only a corrupted *serving store* was tested (the server refused to send); a malicious sender of bad bytes was **not** tested |
| Publisher authenticity of metadata | implemented and tested (Ed25519, pinned key) |
| Rollback / equivocation rejection vs. installed version | implemented and tested (CLI tests; equivocation via a forked publisher root) |
| Freshness / freeze-attack resistance | **not provided** |
| Confidentiality of content | **not provided**: public-data prototype |
| Access control (who may fetch) | **not provided**: any peer reaching a server can fetch by hash or feed id |
| Anonymity / metadata privacy | **not provided**: n0 mode publishes IP addresses via pkarr/DNS, and tickets reveal IP addresses |
| Key rotation / revocation | not implemented |
