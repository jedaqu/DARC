# DARC

**DARC — Deduplicating Archive & Compression**

DARC is an experimental open-source project exploring reliable file archiving and compression.

The public repository contains only material intended for publication. Private research, secrets, internal strategy, and continuity material are maintained outside the public repository.

## Current status

Phase 2A — deterministic content identity with SHA-256-backed in-memory deduplication.

The production implementation is not yet established. Design and implementation will evolve through reproducible tests and benchmarks.

## Repository policy

- Reproducible builds and tests where practical.
- Public documentation describes only the intended public project surface.

## Development

The project is intended to use Rust for the production implementation. The current local research environment also contains a Python reference lab for validating ideas before they are promoted into the public implementation.
