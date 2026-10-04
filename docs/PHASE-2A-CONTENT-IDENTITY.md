# DARC Phase 2A — Content Identity

Phase 2A replaces insertion-order object identities with deterministic content identities.

## Scope

The in-memory object store now derives each object's identity from the SHA-256 digest of its exact bytes. The identity remains explicitly bound to the object store.

This establishes the foundation required for later persistence and portable references.

## Guarantees

- Equal bytes in the same logical store produce the same ObjectId.
- Insertion order does not affect the content-derived digest.
- Different store identities remain distinct even when their bytes are equal.
- Object retrieval still validates the store boundary.
- A detected digest collision is returned as an explicit error instead of silently aliasing different bytes.
- Object payloads remain immutable after insertion.

The implementation uses the RustCrypto sha2 crate, release 0.11.0, for SHA-256.

## Deliberate non-goals

Phase 2A does not yet define a persistent on-disk format, serialization format, garbage collection policy, compression codec, or archive container.

Those concerns remain separate and will be introduced only after content identity is validated.
