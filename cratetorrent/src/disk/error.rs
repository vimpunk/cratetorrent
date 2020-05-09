use std::fmt;

use crate::error::Error;

/// The disk IO result type.
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// Error type returned on failed block writes.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// The block's piece index is invalid.
    InvalidPieceIndex,
    /// An IO error ocurred.
    Io(std::io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidPieceIndex => write!(fmt, "invalid piece index"),
            Self::Io(e) => write!(fmt, "{}", e),
        }
    }
}

/// Error type returned on failed torrent allocations.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug)]
pub(crate) enum NewTorrentError {
    /// The torrent entry already exists in `Disk`'s hashmap of torrents.
    AlreadyExists,
    /// IO error while allocating torrent.
    Io(std::io::Error),
}

impl fmt::Display for NewTorrentError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::AlreadyExists => {
                write!(fmt, "disk torrent entry alrady exists")
            }
            Self::Io(e) => write!(fmt, "{}", e),
        }
    }
}
