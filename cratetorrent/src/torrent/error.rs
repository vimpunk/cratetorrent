use std::fmt;

pub use tokio::{io::Error as IoError, sync::mpsc::error::SendError};

pub(crate) type Result<T, E = TorrentError> = std::result::Result<T, E>;

/// Error type returned on failed block reads.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug)]
#[non_exhaustive]
pub enum TorrentError {
    /// The channel on which some component in engine was listening or sending
    /// died.
    Channel,
    /// An IO error ocurred.
    Io(std::io::Error),
}

impl fmt::Display for TorrentError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        use TorrentError::*;
        match self {
            Channel => write!(fmt, "channel error"),
            Io(e) => write!(fmt, "{}", e),
        }
    }
}

impl From<IoError> for TorrentError {
    fn from(e: IoError) -> Self {
        // the pieces field is a concatenation of 20 byte SHA-1 hashes, so it
        // must be a multiple of 20
        Self::Io(e)
    }
}

impl<T> From<SendError<T>> for TorrentError {
    fn from(_: SendError<T>) -> Self {
        Self::Channel
    }
}
