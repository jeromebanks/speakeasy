# Agent instructions

## Read first

Read README.md and INIT.md before proposing architecture or writing code. Inspect existing files and git state. INIT.md contains project scope, constraints, and milestone acceptance criteria; distinguish those from candidate implementations and unresolved questions.

## Keep the scope small

- Build the standalone P2P publishing layer. Keep payload interpretation, collectors, AI curation, and content search/graph indexes outside it. Publication metadata catalogs and their local indexes belong inside Speakeasy.
- Demonstrate actual peer replication early. Prefer established libraries and a small CLI to a platform/control plane.
- Follow the milestones in INIT.md. Finish a useful vertical slice before expanding infrastructure or the roadmap.
- A Mac mini is the initial seed. No required owner-operated cloud services, paid infrastructure, blockchain, or external database servers.
- Use current official sources when selecting dependencies; document networking and licensing tradeoffs. Iroh/iroh-blobs is the preferred initial trial; pin tested versions. The publication contract remains independent.
- Avoid invented API capabilities, security guarantees, protocol formats, or payload semantics. Say what is unknown and test it.

## Product boundaries

- The first workload is periodic opaque venue/event dataset distribution to multiple consumers. Keep client application names, schemas, and implementation details out of this repository.
- Provide a minimal local publication catalog and machine-readable CLI; keep metadata indexing distinct from payload indexing.
- Preserve versioned formats, configurable runtime/storage/peer settings, resource bounds, and publisher/serving-peer identity separation for later enterprise policy and audit integration. Do not build enterprise infrastructure in the MVP.
- Follow the first-session scope in INIT.md. Do not expand into private communities, distributed computation, multiple backends, or automatic network topology discovery.
- Subsequent review must map acceptance criteria to evidence and independently reproduce checks at the handoff commit.

## Work discipline

- Keep changes focused and reviewable. Preserve unrelated work and inspect diffs before committing.
- Use issue-linked branches when an issue exists. The repository does not yet have an installed SDLC skill; do not assume `work-issue`, review agents, or CI are available.
- Implement the current requested slice with meaningful verification. Record architectural decisions with rationale and consequences, not conversation transcripts.
- Do not create issues, publish releases, change other repos, deploy services, or alter repository access unless requested. Ordinary local implementation and verification may proceed within the agreed scope.
- Keep CLAUDE.md as an import of this file; do not maintain divergent agent policies.

## Verification

Once a Rust crate exists, run formatting checks, linting, and tests appropriate to the change, using the commands supported by that project. Before a crate exists, do not report Rust checks as having run. Keep setup and repeatable verification commands documented.

Exercise behavior that can break the publishing contract: signature/hash validation, unauthorized inputs, replay/rollback, interrupted transfer, atomic install/recovery, idempotent sync, path safety, offline reads, and reseeding with the publisher absent. Add private-publication tests when that feature exists. Use deterministic fixtures and temporary data roots.

Separate local-process tests from actual multi-machine/network evidence. Report NAT/relay failures and dependencies plainly. Do not mark an acceptance step done based only on a mock or simulation.

## Data and security boundaries

- Commit code, documentation, and synthetic fixtures only. Never commit real private events, community membership, invites, signing/decryption keys, recordings, or runtime caches.
- No automatic public discovery or redistribution of confidential feeds.
- A hash proves byte integrity; a signature authenticates a publisher; neither proves factual correctness or the newest available state.
- Use established cryptographic primitives. Define exact signed bytes, trust bootstrap, key lifecycle, and update validation before claiming secure publishing.
- Confidentiality, access control, and anonymity are different properties. Document which are implemented, tested, or deferred.

## Handoff

State what changed, which milestone criteria are satisfied, the exact checks run, remaining limitations, and the next unfinished step. Update INIT.md when an owner decision changes its constraints; add decision records for implementation choices. Never claim remote creation, push, deployment, or tests succeeded without verifying the result.
