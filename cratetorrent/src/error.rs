pub use serde_bencode::Error as BencodeError;
pub use tokio::io::Error as IoError;

use std::convert::From;
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Holds bencode serialization or deserialization related errors.
    Bencode(BencodeError),
    // TODO: add detail
    Disk,
    /// The block length is not 4 KiB.
    InvalidBlockLength,
    /// The torrent download location is not valid.
    // TODO: consider adding more variations (path exists, doesn't exist,
    // permission issues)
    InvalidDownloadPath,
    /// The torrent metainfo is not valid.
    InvalidMetainfo,
    /// Peer's torrent info hash did not match ours.
    InvalidPeerInfoHash,
    /// The piece index was larger than the number of pieces in torrent.
    InvalidPieceIndex,
    /// The chain of piece hashes in the torrent metainfo file was not
    /// a multiple of 20, or is otherwise invalid and thus the torrent could not
    /// be started.
    InvalidPieces,
    /// The bitfield message was not sent after the handshake. According to the
    /// protocol, it should only be accepted after the handshake and when
    /// received at any other time, connection is severed.
    BitfieldNotAfterHandshake,
    /// Only downloads are supported, so the peer we connect to must be a seed.
    /// This error variant is expected to be removed soon, so it should not be
    /// relied upon.
    PeerNotSeed,
    /// Holds IO related errors.
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
