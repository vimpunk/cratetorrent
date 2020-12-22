use std::fmt;

pub use tokio::{io::Error as IoError, sync::mpsc::error::SendError};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    /// The channel on which some component in engine was listening or sending
    /// died.
    Channel,
    /// The torrent download location is not valid.
    // TODO: consider adding more variations (path exists, doesn't exist,
    // permission issues)
    InvalidDownloadPath,
    /// The torrent ID did not correspond to any entry.
    InvalidTorrentId,
    /// Holds IO related errors.
    Io(IoError),
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            Channel => write!(fmt, "channel error"),
            InvalidDownloadPath => write!(fmt, "invalid download path"),
            InvalidTorrentId => write!(fmt, "invalid torrent id"),
            Io(e) => e.fmt(fmt),
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
