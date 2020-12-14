use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{torrent::TorrentStats, TorrentId};

pub(crate) type AlertSender = UnboundedSender<Alert>;
pub type AlertReceiver = UnboundedReceiver<Alert>;

/// The alerts that the engine may send the library user.
pub enum Alert {
    /// Posted when the torrent has finished downloading.
    TorrentComplete(TorrentId),
    /// Each running torrent sends a stats update every second via this alert.
    TorrentStats { id: TorrentId, stats: TorrentStats },
}
