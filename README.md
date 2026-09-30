# Speakeasy

**Publish locally. Share by invitation.**

Speakeasy is a proposed local-first, peer-to-peer content publishing system. Publishers produce versioned artifacts; subscribers fetch and verify them, keep local copies, and can help seed them to other peers. Applications consume those copies without requiring a central service or running AI themselves.

The immediate workload is distributing periodically compiled venue/event datasets to multiple independent consumers. Speakeasy treats payloads as opaque bytes and supports a local publication metadata catalog. Payload schemas, curation, semantic validation, and content lookup indexes belong to producers and consumers. Other knowledge and analytical artifacts can use the same boundary.

This repository currently contains the project brief and agent instructions. Iroh with iroh-blobs is the preferred initial networking trial; no networking or security implementation has been built.

Start with [INIT.md](INIT.md). Agent contributors must follow [AGENTS.md](AGENTS.md); [CLAUDE.md](CLAUDE.md) imports those instructions for Claude Code.

Enterprise evolution should preserve stable identities, versioned formats, configurable deployment settings, and boundaries for later policy and audit integration. The MVP adds no enterprise control plane or mandatory cloud service.

## Brand

Speakeasy evokes communities that share useful information through personal connections. The proposed visual direction is a small illuminated doorway and a discreet invitation card: warm amber, charcoal, simple typography. Do not use blockchain or anonymity claims in the branding. The working name was chosen by the project owner; trademark, package, and domain availability have not been established.

## First milestone

Publish a synthetic dataset on one machine, synchronize it to another over a real peer connection, then demonstrate that the second machine can serve it to a third while the original publisher is offline. Updates must be verified and installed atomically; readers must work offline.

Private publications and cryptographic invitations are the next stage. The public-data prototype must never be presented as suitable for confidential community information.

GitHub stores code, documentation, and synthetic fixtures. It is not the publication store for real events or community membership.
