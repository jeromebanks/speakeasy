# Speakeasy: initial project brief

Status: bootstrap/design only. Owner-approved working name: Speakeasy.

## Problem and purpose

Twiddle has two nascent experiences: Dial for internet radio and Scene (also called Shows) for local venue calendars and music discovery. Gathering their knowledge requires heterogeneous collectors, identity resolution, and AI/human curation. Repeating that work on every user's machine is expensive and wasteful.

Twiddle issue [#7](https://github.com/jeromebanks/twiddle/issues/7) is the integration reference for moving Scene collection into a scheduled local dataset builder. Its current contract must be inspected before integration; this document does not assert a schema or implementation state.

Speakeasy is a separate project for distributing the resulting publications. A publisher does the expensive work once; subscribers retain selected datasets locally and read them without AI or an online knowledge API. Eventually this should support other content and communities too.

The owner's always-on Mac mini is the initial publisher/seed. More seeds can follow. Avoid required infrastructure that the owner must maintain or pay for. A single initial seed is a practical starting topology, not a claim that availability is already decentralized.

Some underground communities intentionally limit visibility. Do not publish real event data, locations, or membership on GitHub or automatically advertise them through a public discovery system. Public source availability does not automatically authorize broader redistribution.

## Boundaries

| Responsibility | Owner |
| --- | --- |
| Source collectors, scheduling, AI/human curation, entity resolution | Twiddle or a separate knowledge producer |
| Event/venue schemas, OKF interpretation, search and graph indexes | Knowledge runtime / consumer |
| Opaque artifacts, manifests, verification, local cache, publication subscriptions, replication | Speakeasy |
| Presentation, playback, personal preferences | Twiddle applications |

OKF is an intended knowledge representation, not Speakeasy's storage schema. Its exact format is not supplied here: do not invent it. Accept a file or directory export through a documented boundary. Consumers should see only complete validated versions at stable local locations. Scene must not import the networking implementation.

## Established constraints

- Local-first consumption, including offline access to the last verified publication.
- Real P2P early: an HTTP download alone does not complete the first milestone.
- No mandatory centralized knowledge service, hosted database, blockchain, token economy, or paid infrastructure.
- No general-purpose filesystem, multiwriter collaboration engine, or Nightshift implementation in the MVP.
- One authoritative signing publisher per feed initially. Subscribers may relay the publisher's bytes without becoming authors.
- Immutable content artifacts and authenticated versioned publication metadata. Choose exact formats after the feasibility spike.
- Transport must not interpret concerts, artists, or OKF. Keep a small seam between local storage/verification and replication; avoid speculative framework design.
- Do not recreate established cryptographic or networking primitives.

Rust is the preferred starting language, subject to a short ecosystem feasibility check. macOS on Apple Silicon is the first operational target; Linux compatibility is desirable. Do not pretend ecosystem suitability has already been established.

## Milestone 0: trial Iroh for the initial networking implementation

Owner direction (2026-09-29): strongly favor [Iroh](https://github.com/n0-computer/iroh) for the initial networking layer, with [iroh-blobs](https://github.com/n0-computer/iroh-blobs) for opaque artifact transfer. Speakeasy is itself a prototype; a dependency's pre-production status is not, by itself, a reason to postpone the experiment. Try it with synthetic data, measure its behavior, and determine whether it works for us.

Begin with a short check of current official documentation, compatible crate versions, licensing, and macOS support, then build a tiny Rust connectivity and blob-transfer spike. Pin the tested versions. Document maturity limitations and observed failures without turning the dependency review into a production-readiness gate.

Iroh is the preferred trial implementation, not an irrevocable protocol commitment. Keep publication identity, version metadata, artifact manifests, and the verified local export boundary independent of the networking implementation. Do not build a universal protocol abstraction or multiple backends now. Iroh endpoint authentication and blob integrity do not replace publisher-signed publication metadata, community authorization, or encryption of stored private publications.

Use explicitly exchanged peer addresses/tickets for the first trial where practical. Document discovery and NAT traversal behavior, which transfers are direct or relayed, and any external bootstrap/relay dependencies and costs. Free shared connectivity infrastructure is acceptable for the trial; it does not constitute a centralized knowledge service. Keep the constraints against mandatory paid infrastructure and owner-operated cloud services.

Prove transfer from A to B and subsequent serving from B to C with A offline. Exercise interrupted transfer/resumption and verification of received bytes. Verify connectivity across two real machines/networks before claiming internet connectivity; local-process tests prove only local behavior. If the execution environment cannot access those machines, complete the local spike and provide exact commands for the owner to run, marking the external-network criteria unverified.

Write `docs/transport-decision.md` alongside the spike with tested versions, setup and verification commands, observed results, remaining limitations, and a short plan for Milestone 1. Avoid a broad comparative survey before trying Iroh. Revisit Hypercore, libp2p, Willow, or existing publishing systems only if the trial exposes a concrete blocker or material mismatch; record the reason before changing direction. A successful trial should lead directly into the smallest useful Iroh-based implementation slice.

## Milestone 1: verified publication and peer replication

Use fictional, non-sensitive payloads. The intended command concepts are publish, serve/seed, subscribe, sync, status, and export; exact CLI syntax is an implementation decision.

Minimum behavior:

1. A publishes version 1 of an opaque fixture. A persistent publisher identity authenticates its publication metadata.
2. B subscribes through an explicitly shared descriptor that pins the publisher identity. A matching hash alone is insufficient to establish authenticity or trust.
3. B transfers over a peer connection, verifies metadata and payload, and exposes the complete local version. Define precisely which bytes the signatures and hashes cover; do not rely on incidental JSON serialization.
4. A publishes version 2 with one changed artifact. B fetches only missing artifacts where the chosen backend supports this; document transfer granularity rather than promising arbitrary byte-level deltas.
5. Interrupted transfer, invalid signatures, corrupted content, and missing artifacts preserve B's previous complete version. Persist version installation atomically and recover after restart.
6. A goes offline. B continues local reads and serves the verified publication to C. C independently verifies the original publisher's metadata.
7. Repeating sync is idempotent. Status reports the installed version, last successful synchronization, and freshness metadata without treating a failed check as a successful update.

Authenticated version metadata should identify the feed, publisher, version/sequence, artifact hashes and sizes, schema/protocol version, and publication time. Define rollback protection against older versions than a client has already accepted. Signatures cannot establish that a client has received the newest version; expose this limitation. Publication timestamps indicate publisher claims, not proof of freshness.

Use bounded artifact sizes, paths, and disk usage. Reject path traversal and unsafe extraction before installation. Keep secrets out of descriptors, logs, fixtures, and commits. Document publisher key storage and backup expectations. Garbage collection must preserve installed versions and in-progress operations; advanced storage policies can wait.

No application-specific query API is required. Expose a verified local snapshot/export plus status. Consumers rebuild their own disposable lookup indexes and decide how to present stale data.

## Milestone 2: simple private publications

Design this boundary now; implement after the basic exchange works. Begin with publisher-issued invitations, encrypted artifacts, and explicit recipient identity/key binding. Keep confidential payloads out of the public prototype until this is tested.

An invite must distinguish permission to subscribe from permission to decrypt. Specify how keys reach authorized recipients, what seeders can see, and whether unauthorized peers can discover feed identifiers or update timing. Encrypt content before publishing it to a network accessible to untrusted peers. Do not claim anonymity or protection from authorized recipients redistributing plaintext.

Revocation concerns future content: removed readers may retain already decrypted information. Test key rotation and define the authority/freshness needed to enforce revocation. Offline operation and immediate global revocation cannot simply be assumed to coexist.

## Later: delegated communities

The motivating example is an invitation-only community where some members can grant read access and delegate narrower invitation rights. Investigate signed, attenuable capabilities and established authorization protocols before designing a new scheme. No blockchain requirement.

Record unresolved issues rather than smuggling them into MVP scope: delegation depth, expiry, revocation chains, key recovery, abuse, membership privacy, and invite quotas. A token saying “two invites” cannot by itself enforce a global quota across independent offline peers; that needs a redemption/coordination model. Neither encryption nor unlisted feeds prevents an authorized member leaking an event location.

Also defer multi-publisher conflict resolution, distributed curation, graph merging, general filesystem semantics, global directories, moderation systems, and token incentives.

## First Claude Code session

Read AGENTS.md and this brief. Inspect the repo's actual state. Begin Milestone 0 with the preferred Iroh/iroh-blobs trial, using current primary sources and small experiments. Record the trial evidence and concrete implementation plan, then proceed within the agreed constraints to the smallest useful Iroh-based slice if the results support it. Do not create a large backlog before demonstrating transfer. Do not edit Twiddle or create external issues unless explicitly requested. Report evidence, limitations, and the next unfinished milestone at handoff.

## Repository status

Repository: [jeromebanks/speakeasy](https://github.com/jeromebanks/speakeasy). The owner created it as a public repository. Public code hosting does not determine publication visibility: real events, membership, secrets, and runtime datasets stay outside this repository.

This bootstrap contains documentation and agent instructions only. Licensing is undecided; select a license before presenting the project as open source.
