use std::time::{Duration, Instant};

use crate::{counter::Counter, PieceIndex};

/// Aggregated statistics of a torrent.
#[derive(Clone, Debug, Default)]
pub struct TorrentStats {
    /// When the torrent was _first_ started.
    pub start_time: Option<Instant>,
    /// How long the torrent has been running.
    pub run_duration: Duration,

    /// The pieces this torrent has.
    pub pieces: PieceStats,

    pub downloaded_payload_stats: ThroughputStats,
    pub wasted_payload_count: u64,
    pub uploaded_payload_stats: ThroughputStats,
    pub downloaded_protocol_stats: ThroughputStats,
    pub uploaded_protocol_stats: ThroughputStats,

    pub peer_count: usize,
    // TODO: include latest errors, if any
}

/// Statistics of a torrent's pieces.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PieceStats {
    /// The total number of pieces in torrent.
    pub total: usize,
    /// The number of pieces that the torrent is currently downloading.
    pub pending: usize,
    /// The number of pieces that the torrent has downloaded.
    pub complete: usize,
    /// The pieces that were completed since the last status update.
    ///
    /// By default this information is not sent, as it has a little overhead. It
    /// needs to be turned on in the torrent's [configuration]
    /// (crate::conf::TorrentAlertConf::latest_completed_pieces).
    pub latest_completed: Option<Vec<PieceIndex>>,
}

impl PieceStats {
    /// Returns whether the torrent is a seed.
    pub fn is_seed(&self) -> bool {
        self.complete == self.total
    }

    /// Returns whether the torrent is in endgame mode (about to finish
    /// download).
    pub fn is_in_endgame(&self) -> bool {
        self.pending + self.complete == self.total
    }
}

/// Statistics of a torrent's current throughput.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThroughputStats {
    pub total: u64,
    pub rate: u64,
    pub peak: u64,
}

impl From<&Counter> for ThroughputStats {
    fn from(c: &Counter) -> Self {
        Self {
            total: c.total(),
            rate: c.avg(),
            peak: c.peak(),
        }
    }
}
