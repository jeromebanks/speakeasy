# Known issues

Source: a Codex code review of `93294b6..832d500` (branch `m0-iroh-spike`),
found by reading the code. Fixed items have regression tests. Open items are
tracked on GitHub and have **not** been reproduced unless noted.

## Fixed

| Finding | Fix | Test |
| --- | --- | --- |
| **High:** an error after the commit point let sync try another peer and compare against stale state. A conflicting signed version with the same sequence could then replace the live directory. | `install` and `place_version` refuse any sequence ≤ the committed `current`, checked *before* tagging. A local install error stops the peer loop, runs recovery and exits 1. | `tests/install.rs` crash test: a conflicting v2 is rejected while `state.json` lags `current` |
| **High:** race between the publish-input symlink check and the import. A file swapped for a symlink (for example to `publisher.key`) after listing would have been followed. | `src/input.rs` opens every component with `openat` and `O_NOFOLLOW`, checks with `fstat` that it is a regular file, and imports from that handle via a stream. Input that overlaps the runtime root is refused. | `input::tests::rejects_symlinked_files_and_directories` (file swap and directory swap) |
| **High:** head requests had no lifetime or concurrency limits. | At most 64 concurrent head exchanges (excess connections are closed as busy), a 5 s deadline covering stream accept, request and response, and a 2 s linger. | `tests/install.rs` `idle_head_connections_are_closed_by_the_server` |
| **Medium:** relative `publish --from` paths failed ("path must be absolute"). | Fixed by the stream-based import above. | checked manually: `--from ./fx` publishes |

## Open

| Severity | Issue | Summary |
| --- | --- | --- |
| medium | [#1](https://github.com/jeromebanks/speakeasy/issues/1) | Declared artifact sizes don't limit the bytes actually fetched or exported during sync. |
| medium | [#2](https://github.com/jeromebanks/speakeasy/issues/2) | Parent directories are not fsynced when feeds and key files are created. |
| medium | [#3](https://github.com/jeromebanks/speakeasy/issues/3) | `status`, `export` and `inspect` can show stale state after a crash at the commit point, until recovery runs. |
| medium | [#4](https://github.com/jeromebanks/speakeasy/issues/4) | Recovery and pruning can leave manifests and blob tags behind; GC is off. |
| medium | [#5](https://github.com/jeromebanks/speakeasy/issues/5) | `serve --sync-every` doesn't handle shutdown while a sync is running. |
| medium | [#6](https://github.com/jeromebanks/speakeasy/issues/6) | `init` and `subscribe` write metadata without the root lock. |
| low | [#7](https://github.com/jeromebanks/speakeasy/issues/7) | Some verification failures exit with 1 or 4 instead of 3. |
| test gap | [#8](https://github.com/jeromebanks/speakeasy/issues/8) | No crash tests with real process kills; no test for a wrong hash with the right size. |

The iroh-blobs protocol handler (`iroh_blobs::ALPN`) has no application-level
concurrency cap. Idle connections are bounded only by QUIC's idle timeout.
