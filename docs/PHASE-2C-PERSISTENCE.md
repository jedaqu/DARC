# Phase 2C — Persistent StateRoot snapshots

## Scope

Phase 2C adds the logical persistence boundary for DARC `StateRoot`.

It persists:

- the `ObjectStoreId` that owns the logical state;
- each logical entry name;
- the SHA-256-backed `ObjectId` digest referenced by that entry.

Object payloads are not copied into the state snapshot. They remain owned by the persistent `ObjectStore` boundary established in Phase 2B.

## Snapshot format

Current format:

`MAGIC | VERSION | STORE_ID | ENTRY_COUNT | [NAME_LENGTH | NAME_BYTES | OBJECT_DIGEST]...`

Current constants:

- magic: `DARCST01`;
- version: `1`;
- store ID: 128-bit little-endian value;
- entry count: 64-bit little-endian value;
- name length: 64-bit little-endian value;
- name bytes: UTF-8;
- object digest: 32-byte SHA-256 value.

Entries are serialized in lexical name order. Therefore, equivalent logical roots serialize to identical bytes even when entries were inserted in a different order.

## Load-time verification

Loading a state snapshot is bound to an existing `ObjectStore` and verifies:

- magic;
- supported version;
- complete header and records;
- store identity;
- UTF-8 validity of entry names;
- object existence in the target store;
- duplicate entry names;
- unexpected trailing bytes;
- integer and length overflow conditions.

A snapshot that references another store or an object that is not present in the target store is rejected.

## API

The public core exposes:

- `StateRoot::write_snapshot`;
- `StateRoot::read_snapshot`;
- `StateRootPersistenceError`.

The API operates over standard `Read` / `Write` traits so the logical-state boundary remains independent from filesystem layout and the final archive container.

## Deliberate non-goals

This phase does not define:

- compression;
- the final archive/container format;
- block-level storage redesign;
- root history or version chains;
- root hashing as a new identity system;
- CLI behavior;
- network behavior;
- path normalization policy.
