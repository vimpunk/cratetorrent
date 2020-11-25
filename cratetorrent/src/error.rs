use std::{convert::From, fmt};

pub use {
    serde_bencode::Error as BencodeError,
    tokio::{io::Error as IoError, sync::mpsc::error::SendError},
};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    /// Holds bencode serialization or deserialization related errors.
    Bencode(BencodeError),
    /// The channel on which some component in engine was listening or sending
    /// died.
    Channel,
    /// Peers are not allowed to request blocks while they are choked. If they
    /// do so, their connection is severed.
    ChokedPeerSentRequest,
    /// The bitfield contained a different number of pieces than our own.
    InvalidBitfield,
    /// The block length is not 16 KiB.
    InvalidBlockInfo,
    /// The torrent download location is not valid.
    // TODO: consider adding more variations (path exists, doesn't exist,
    // permission issues)
    InvalidDownloadPath,
    /// The torrent metainfo is not valid.
    InvalidMetainfo,
    /// The torrent ID did not correspond to any entry.
    InvalidTorrentId,
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

impl<T> From<SendError<T>> for Error {
    fn from(_: SendError<T>) -> Self {
        Self::Channel
    }
}

impl From<BencodeError> for Error {
    fn from(e: BencodeError) -> Self {
        Self::Bencode(e)
    }
}
