# Phase 2B — Persistent ObjectStore snapshots

## Scope

Phase 2B adds the first persistent boundary for the DARC object store.

It persists:

- the logical `ObjectStoreId`;
- each immutable object's SHA-256 content identity;
- the original, uncompressed object bytes.

Compression, the final archive container, a CLI, and network protocols remain outside this phase.

## Snapshot format

Current format:

`MAGIC | VERSION | STORE_ID | OBJECT_COUNT | [DIGEST | LENGTH | BYTES]...`

Current constants:

- magic: `DARCOS01`;
- version: `1`;
- store ID: 128-bit little-endian value;
- object count: 64-bit little-endian value;
- digest: 32-byte SHA-256 value;
- object length: 64-bit little-endian value;
- payload: original object bytes, without compression.

Objects are written in deterministic object-ID order. Therefore, two stores with the same logical store ID and the same objects serialize to identical bytes even when their insertion order differs.

## Load-time verification

Loading a snapshot verifies:

- magic;
- supported version;
- complete header and records;
- each stored digest against the payload bytes;
- duplicate object records;
- absence of unexpected trailing bytes.

A corrupted or incompatible snapshot is rejected rather than silently accepted.

## API

The public core exposes:

- `ObjectStore::write_snapshot`;
- `ObjectStore::read_snapshot`;
- `ObjectStorePersistenceError`.

The API operates over standard `Read` / `Write` traits so the persistence boundary is not tied to a filesystem or archive container.

## Deliberate non-goals

This phase does not define:

- compression;
- archive layout;
- file/directory tree persistence;
- a persistent StateRoot format;
- indexing or performance optimization;
- CLI or network behavior.

Those remain later decisions subject to their own pre-audits.
