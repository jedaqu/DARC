use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

use crate::DarcError;

pub trait Codec: Clone {
    fn encode(&self, input: &[u8]) -> Result<Vec<u8>, DarcError>;
    fn decode(&self, input: &[u8], expected_size: usize) -> Result<Vec<u8>, DarcError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ZlibCodec;

impl Codec for ZlibCodec {
    fn encode(&self, input: &[u8]) -> Result<Vec<u8>, DarcError> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(input)
            .map_err(|error| DarcError::CodecFailure(error.to_string()))?;
        encoder
            .finish()
            .map_err(|error| DarcError::CodecFailure(error.to_string()))
    }

    fn decode(&self, input: &[u8], expected_size: usize) -> Result<Vec<u8>, DarcError> {
        let mut decoder = ZlibDecoder::new(input);
        let mut output = Vec::with_capacity(expected_size);
        decoder
            .read_to_end(&mut output)
            .map_err(|error| DarcError::CodecFailure(error.to_string()))?;
        if output.len() != expected_size {
            return Err(DarcError::SizeMismatch);
        }
        Ok(output)
    }
}

pub(crate) fn digest(input: &[u8]) -> [u8; 32] {
    Sha256::digest(input).into()
}
