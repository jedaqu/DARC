//! Codec boundary for transforming original object bytes into payload bytes.
//!
//! This module deliberately provides a reference, non-compressing codec only.
//! The production compression algorithm remains a separate decision that
//! requires additional corpus evidence and dependency review.

use std::fmt;

use super::sha256;

/// Stable identifier for a codec family and its format version.
///
/// The identifier is carried by EncodedPayload so a future persistent
/// payload can identify the decoder contract without coupling the codec to the
/// final archive/container format.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CodecId {
    family: u16,
    version: u16,
}

impl CodecId {
    pub const fn new(family: u16, version: u16) -> Self {
        Self { family, version }
    }

    pub const fn family(self) -> u16 {
        self.family
    }

    pub const fn version(self) -> u16 {
        self.version
    }
}

impl fmt::Display for CodecId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.family, self.version)
    }
}

/// Reference codec identifier.
///
/// This codec preserves the original bytes without compression. It exists to
/// exercise the boundary and integrity contract while production codec
/// selection remains open.
pub const IDENTITY_CODEC_V1: CodecId = CodecId::new(0, 1);

/// Encoded payload produced by a Codec.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedPayload {
    codec: CodecId,
    bytes: Vec<u8>,
}

impl EncodedPayload {
    pub fn new(codec: CodecId, bytes: Vec<u8>) -> Self {
        Self { codec, bytes }
    }

    pub const fn codec(&self) -> CodecId {
        self.codec
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Errors raised at the codec boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodecError {
    CodecMismatch {
        expected: CodecId,
        actual: CodecId,
    },
    EncodeFailed(String),
    DecodeFailed(String),
    IntegrityMismatch {
        expected: [u8; 32],
        actual: [u8; 32],
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodecMismatch { expected, actual } => {
                write!(formatter, "codec mismatch: expected {expected}, got {actual}")
            }
            Self::EncodeFailed(message) => write!(formatter, "codec encode failed: {message}"),
            Self::DecodeFailed(message) => write!(formatter, "codec decode failed: {message}"),
            Self::IntegrityMismatch { .. } => write!(formatter, "decoded payload digest mismatch"),
        }
    }
}

impl std::error::Error for CodecError {}

/// A codec transforms original bytes into an explicitly identified payload
/// and restores the original bytes from that payload.
pub trait Codec {
    fn id(&self) -> CodecId;

    fn encode(&self, input: &[u8]) -> Result<EncodedPayload, CodecError>;

    fn decode(&self, payload: &EncodedPayload) -> Result<Vec<u8>, CodecError>;
}

/// Reference codec used to validate the boundary without selecting a
/// production compression algorithm.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityCodec;

impl Codec for IdentityCodec {
    fn id(&self) -> CodecId {
        IDENTITY_CODEC_V1
    }

    fn encode(&self, input: &[u8]) -> Result<EncodedPayload, CodecError> {
        Ok(EncodedPayload::new(self.id(), input.to_vec()))
    }

    fn decode(&self, payload: &EncodedPayload) -> Result<Vec<u8>, CodecError> {
        if payload.codec() != self.id() {
            return Err(CodecError::CodecMismatch {
                expected: self.id(),
                actual: payload.codec(),
            });
        }

        Ok(payload.bytes().to_vec())
    }
}

/// Decode a payload and verify that the restored bytes still match the
/// original content identity.
pub fn decode_verified<C: Codec>(
    codec: &C,
    payload: &EncodedPayload,
    expected_digest: [u8; 32],
) -> Result<Vec<u8>, CodecError> {
    let decoded = codec.decode(payload)?;
    let actual = sha256(&decoded);

    if actual != expected_digest {
        return Err(CodecError::IntegrityMismatch {
            expected: expected_digest,
            actual,
        });
    }

    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_codec_round_trip_is_exact() {
        let codec = IdentityCodec;
        let original = b"repeatable payload bytes";

        let payload = codec.encode(original).expect("encode succeeds");
        let decoded = codec.decode(&payload).expect("decode succeeds");

        assert_eq!(payload.codec(), IDENTITY_CODEC_V1);
        assert_eq!(decoded, original);
    }

    #[test]
    fn identity_codec_is_deterministic() {
        let codec = IdentityCodec;
        let original = b"deterministic";

        let first = codec.encode(original).expect("encode succeeds");
        let second = codec.encode(original).expect("encode succeeds");

        assert_eq!(first, second);
    }

    #[test]
    fn decoder_rejects_a_payload_for_another_codec() {
        let codec = IdentityCodec;
        let payload = EncodedPayload::new(CodecId::new(99, 1), b"data".to_vec());

        assert_eq!(
            codec.decode(&payload),
            Err(CodecError::CodecMismatch {
                expected: IDENTITY_CODEC_V1,
                actual: CodecId::new(99, 1),
            })
        );
    }

    #[test]
    fn verified_decode_preserves_original_content_identity() {
        let codec = IdentityCodec;
        let original = b"identity is based on original bytes";
        let expected_digest = sha256(original);

        let payload = codec.encode(original).expect("encode succeeds");
        let decoded =
            decode_verified(&codec, &payload, expected_digest).expect("verified decode succeeds");

        assert_eq!(sha256(decoded.as_slice()), expected_digest);
        assert_eq!(decoded, original);
    }

    #[test]
    fn verified_decode_rejects_corrupted_payload() {
        let codec = IdentityCodec;
        let original = b"integrity matters";
        let expected_digest = sha256(original);

        let payload = EncodedPayload::new(codec.id(), b"integrity mutters".to_vec());

        assert!(matches!(
            decode_verified(&codec, &payload, expected_digest),
            Err(CodecError::IntegrityMismatch { expected, .. }) if expected == expected_digest
        ));
    }
}
