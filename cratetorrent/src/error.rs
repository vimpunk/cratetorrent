//! This module includes the errors that could occur in the engine. They are
//! reported via the [alert system](crate::alert), as most operations via the
//! engine happen asynchronously.

use std::{fmt, net::SocketAddr};

use crate::TorrentId;

pub use crate::{
    peer::error::PeerError, torrent::error::TorrentError, tracker::TrackerError,
};
pub use tokio::{io::Error as IoError, sync::mpsc::error::SendError};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The channel on which some component in engine was listening or sending
    /// died.
    Channel,
    /// The torrent download location is not valid.
    // TODO: consider adding more variations (path exists, doesn't exist,
    // permission issues)
    InvalidDownloadPath,
    /// The torrent ID did not correspond to any entry. This is returned when
    /// the user specified a torrent that does not exist.
    InvalidTorrentId,
    /// Holds global IO related errors.
    Io(IoError),
    /// An error specific to a torrent.
    Torrent { id: TorrentId, error: TorrentError },
    /// An error that occurred while a torrent was announcing to tracker.
    Tracker { id: TorrentId, error: TrackerError },
    /// An error that occurred in a torrent's session with a peer.
    Peer {
        id: TorrentId,
        addr: SocketAddr,
        error: PeerError,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            Channel => write!(fmt, "channel error"),
            InvalidDownloadPath => write!(fmt, "invalid download path"),
            InvalidTorrentId => write!(fmt, "invalid torrent id"),
            Io(e) => e.fmt(fmt),
            Torrent { id, error } => {
                write!(fmt, "torrent {} error: {}", id, error)
            }
            Tracker { id, error } => {
                write!(fmt, "torrent {} tracker error: {}", id, error)
            }
            Peer { id, addr, error } => {
                write!(fmt, "torrent {} peer {} error: {}", id, addr, error)
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use Error::*;
        match self {
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
