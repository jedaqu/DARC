use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DarcError {
    WrongStore,
    ObjectNotFound,
    IntegrityFailure,
    CodecFailure(String),
    InvalidPath,
    SizeMismatch,
}

impl fmt::Display for DarcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongStore => write!(f, "object or root belongs to a different store"),
            Self::ObjectNotFound => write!(f, "object not found"),
            Self::IntegrityFailure => write!(f, "object integrity check failed"),
            Self::CodecFailure(message) => write!(f, "codec failure: {message}"),
            Self::InvalidPath => write!(f, "invalid file path"),
            Self::SizeMismatch => write!(f, "materialized file size does not match metadata"),
        }
    }
}

impl std::error::Error for DarcError {}
