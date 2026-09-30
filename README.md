# Speakeasy

**Publish locally. Share by invitation.**

Speakeasy is a proposed local-first, peer-to-peer content publishing system. Publishers produce versioned artifacts; subscribers fetch and verify them, keep local copies, and can help seed them to other peers. Applications consume those copies without requiring a central service or running AI themselves.

The first consumer is [Twiddle](https://github.com/jeromebanks/twiddle): portable, curated venue/event datasets for Scene, followed by radio catalogs and other knowledge artifacts. Speakeasy treats the payload as opaque bytes. Music schemas, OKF interpretation, curation, and application lookup indexes belong outside it.

This repository currently contains the project brief and agent instructions. No networking stack, protocol, or security implementation has been selected or built.

Start with [INIT.md](INIT.md). Agent contributors must follow [AGENTS.md](AGENTS.md); [CLAUDE.md](CLAUDE.md) imports those instructions for Claude Code.

## Brand

Speakeasy evokes communities that share useful information through personal connections. The proposed visual direction is a small illuminated doorway and a discreet invitation card: warm amber, charcoal, simple typography. Do not use blockchain or anonymity claims in the branding. The working name was chosen by the project owner; trademark, package, and domain availability have not been established.

## First milestone

Publish a synthetic dataset on one machine, synchronize it to another over a real peer connection, then demonstrate that the second machine can serve it to a third while the original publisher is offline. Updates must be verified and installed atomically; readers must work offline.

Private publications and cryptographic invitations are the next stage. The public-data prototype must never be presented as suitable for confidential community information.

GitHub stores code, documentation, and synthetic fixtures. It is not the publication store for real events or community membership.
