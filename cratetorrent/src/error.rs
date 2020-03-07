pub use serde_bencode::Error as BencodeError;
pub use tokio::io::Error as IoError;

use std::convert::From;
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Bencode(BencodeError),
    InvalidInfoHash,
    InvalidPieces,
    Io(IoError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            Bencode(e) => write!(f, "{}", e),
            Io(e) => write!(f, "{}", e),
            _ => write!(f, "{:?}", *self),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use Error::*;
        match self {
            Bencode(e) => Some(e),
            Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<IoError> for Error {
    fn from(e: IoError) -> Self {
        // the pieces field is a concatenation of 20 byte SHA-1 hashes, so it
        // must be a multiple of 20
        Self::Io(e)
    }
}

impl From<BencodeError> for Error {
    fn from(e: BencodeError) -> Self {
        Self::Bencode(e)
    }
}
