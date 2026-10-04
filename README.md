# DARC

**DARC — Deduplicating Archive & Compression**

DARC is an experimental open-source project exploring reliable file archiving and compression.

The public repository contains only material intended for publication. Private research, secrets, internal strategy, and continuity material are maintained outside the public repository.

## Current status

Phase 1 — core model.

The first Rust core now provides store-scoped object identities, content interning, state roots, replacement by new roots, integrity verification, and a replaceable codec boundary with a zlib baseline.

Persistence, the final archive format, and further codec research are not part of this phase.

## Repository policy

- No secrets or credentials.
- No private strategy or confidential research notes.
- Public documentation describes only the intended public project surface.

## Development

The production implementation is written in Rust. Private research and offline reference work are maintained outside the public repository.
