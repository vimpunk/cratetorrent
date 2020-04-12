use {
    std::{convert::From, fmt},
    tokio::sync::mpsc::error::SendError,
};

/// The disk IO result type.
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// The unrecoverable errors that may occur on the disk task.
#[derive(Debug)]
pub enum Error {
    /// The specified torrent was not found among the disk task's torrent
    /// entries.
    InvalidTorrentId,
    /// The channel on which disk was listening or sending died.
    Channel,
}

impl<T> From<SendError<T>> for Error {
    fn from(_: SendError<T>) -> Self {
        Self::Channel
    }
}

/// Error type returned on failed block writes.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// The block's piece index is invalid.
    InvalidPieceIndex,
    /// The file download was finished but it could not be moved to its
    /// destination path (removing the `.part` suffix).
    DestPath,
    /// An IO error ocurred.
    Io(std::io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidPieceIndex => write!(fmt, "invalid piece index"),
            Self::DestPath => {
                write!(fmt, "could not move downloaded file to path")
            }
            Self::Io(e) => write!(fmt, "{}", e),
        }
    }
}

/// Error type returned on failed torrent allocations.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug)]
pub(crate) struct NewTorrentError(pub std::io::Error);

impl fmt::Display for NewTorrentError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", self.0)
    }
}
