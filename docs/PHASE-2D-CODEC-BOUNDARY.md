# Phase 2D — Codec boundary

## Purpose

Phase 2D establishes an explicit boundary between original object bytes and any encoded or compressed payload.

The boundary is intentionally independent from:

- ObjectId content identity;
- ObjectStore storage;
- StateRoot logical persistence;
- the final archive/container format.

## Contract

The current public contract is:

`original bytes → codec encode → encoded payload → codec decode → original bytes`

Each encoded payload carries a CodecId composed of a codec family and version.

Content identity remains based on the original bytes. The codec never becomes the source of ObjectId identity.

## Reference implementation

The first implementation provides IdentityCodec.

IdentityCodec performs no compression. Its purpose is to validate:

- deterministic encoding;
- exact round-trip;
- explicit codec identification;
- rejection of a payload belonging to another codec;
- post-decode integrity verification against the original SHA-256 digest.

This is a reference boundary, not a production compression selection.

## Integrity

decode_verified verifies the SHA-256 digest of the restored original bytes.

Therefore a payload can only be accepted as successfully restored when:

1. the payload is decodable by the selected codec; and
2. the restored bytes match the expected original content identity.

## Production algorithm decision

No production compression codec is selected by this implementation.

The next evidence required before freezing a production codec is:

- representative corpus benchmarks;
- encode/decode correctness;
- corruption behavior;
- dependency and maintenance review;
- deterministic behavior where applicable.

## Deliberate non-goals

This implementation does not define:

- a final archive/container;
- automatic multi-codec selection;
- block-level compression;
- a new object identity scheme;
- changes to StateRoot;
- CLI behavior;
- network behavior;
- root history.
