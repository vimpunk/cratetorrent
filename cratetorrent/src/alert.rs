//! This module defines the alerts the API user may receive from the torrent
//! engine.
//!
//! Communication of such alerts is performed via unbounded [tokio mpsc
//! channels](tokio::sync::mpsc). Thus, the application in which the engine is
//! integrated may be driven partially or entirely by cratetorrent alerts.

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{torrent::TorrentStats, TorrentId};

pub(crate) type AlertSender = UnboundedSender<Alert>;
/// The channel on which alerts from the engine can be received. See [`Alert`]
/// for the type of messages that can be received.
pub type AlertReceiver = UnboundedReceiver<Alert>;

/// The alerts that the engine may send the library user.
#[derive(Debug, Clone)]
pub enum Alert {
    /// Posted when the torrent has finished downloading.
    TorrentComplete(TorrentId),
    /// Each running torrent sends an update of its latest statistics every
    /// second via this alert.
    TorrentStats { id: TorrentId, stats: TorrentStats },
}
