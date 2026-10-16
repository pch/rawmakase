//! What can go wrong reading a RAW file.
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a RAW file this crate recognises.
    NotRaw,
    /// A recognised container whose variant (encoding, layout, sensor) is not
    /// decoded here.
    Unsupported(String),
    /// The file says something impossible, or ends early.
    Corrupt(String),
    /// A size in the file is beyond what is decoded (allocation limits).
    TooLarge(&'static str),
    /// The caller asked the decode to stop.
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotRaw => write!(f, "Unsupported file format or not RAW file"),
            Error::Unsupported(what) => write!(f, "Unsupported RAW variant: {what}"),
            Error::Corrupt(what) => write!(f, "Corrupt RAW data: {what}"),
            Error::TooLarge(what) => write!(f, "RAW too large: {what}"),
            Error::Cancelled => write!(f, "Decoding cancelled"),
        }
    }
}

impl std::error::Error for Error {}

/// A `Corrupt` error.
pub(crate) fn corrupt(what: impl Into<String>) -> Error {
    Error::Corrupt(what.into())
}
