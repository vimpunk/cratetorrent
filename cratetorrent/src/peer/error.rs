use std::fmt;

pub use tokio::{io::Error as IoError, sync::mpsc::error::SendError};

pub(crate) type Result<T, E = PeerError> = std::result::Result<T, E>;

/// Error type returned on failed peer sessions.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug)]
#[non_exhaustive]
pub enum PeerError {
    /// The bitfield message was not sent after the handshake. According to the
    /// protocol, it should only be accepted after the handshake and when
    /// received at any other time, connection is severed.
    BitfieldNotAfterHandshake,
    /// The channel on which some component in engine was listening or sending
    /// died.
    Channel,
    /// Peers are not allowed to request blocks while they are choked. If they
    /// do so, their connection is severed.
    RequestWhileChoked,
    /// A peer session timed out because neither side of the connection became
    /// interested in each other.
    InactivityTimeout,
    /// The block information the peer sent is invalid.
    InvalidBlockInfo,
    /// The block's piece index is invalid.
    InvalidPieceIndex,
    /// Peer's torrent info hash did not match ours.
    InvalidInfoHash,
    /// An IO error ocurred.
    Io(std::io::Error),
}

impl fmt::Display for PeerError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use PeerError::*;
        match self {
            BitfieldNotAfterHandshake => {
                write!(fmt, "received unexpected bitfield")
            }
            Channel => write!(fmt, "channel error"),
            RequestWhileChoked => {
                write!(fmt, "choked peer sent request")
            }
            InactivityTimeout => write!(fmt, "inactivity timeout"),
            InvalidBlockInfo => write!(fmt, "invalid block info"),
            InvalidPieceIndex => write!(fmt, "invalid piece index"),
            InvalidInfoHash => write!(fmt, "invalid info hash"),
            Io(e) => write!(fmt, "{}", e),
        }
    }
}

impl From<IoError> for PeerError {
    fn from(e: IoError) -> Self {
        // the pieces field is a concatenation of 20 byte SHA-1 hashes, so it
        // must be a multiple of 20
        Self::Io(e)
    }
}

impl<T> From<SendError<T>> for PeerError {
    fn from(_: SendError<T>) -> Self {
        Self::Channel
    }
}
