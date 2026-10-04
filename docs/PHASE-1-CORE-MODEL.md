# DARC Phase 1 — Core Object Model

Phase 1 establishes the first public Rust implementation of DARC's core state model.

## Scope

The implementation is intentionally in-memory and dependency-free. It establishes the invariants that later persistence, hashing, packing, and codec layers can build on without coupling those concerns together.

The public core contains:

- an ObjectStoreId for store identity;
- opaque ObjectId values bound to a specific store;
- immutable objects interned exactly once per store;
- FileEntry references instead of embedded object bytes;
- immutable StateRoot values that can produce a new root when an object reference is replaced;
- explicit rejection of cross-store references.

## Deliberate non-goals

Phase 1 does not define a disk format, a persistent index, a content hash algorithm, a compression codec, a CLI, or a network protocol.

Those pieces remain separate so they can be evaluated independently.

## Correctness boundary

ObjectStore::intern deduplicates equal byte sequences within one store. ObjectStore::get and StateRoot mutations validate store identity before accepting a reference.

The current index uses the original bytes rather than a digest. This is a deliberately simple correctness-first implementation for the first public phase; a later phase can introduce hashing and persistent storage once the invariants are stable.
