use std::time::{Duration, Instant};

use crate::counter::Counter;

/// Aggregated statistics of a torrent.
#[derive(Clone, Debug)]
pub struct TorrentStats {
    /// When the torrent was _first_ started.
    pub start_time: Option<Instant>,
    /// How long the torrent has been running.
    pub run_duration: Duration,

    /// The pieces this torrent has.
    pub pieces: PieceStats,

    pub download_payload_stats: ThroughputStats,
    pub wasted_payload_count: u64,
    pub upload_payload_stats: ThroughputStats,
    pub download_protocol_stats: ThroughputStats,
    pub upload_protocol_stats: ThroughputStats,

    pub peer_count: usize,
    // TODO: include latest errors, if any
}

/// Statistics a torrent's pieces.
#[derive(Clone, Copy, Debug)]
pub struct PieceStats {
    pub pending: usize,
    pub complete: usize,
    pub total: usize,
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

/// Statistics a torrent's current throughput.
#[derive(Clone, Copy, Debug)]
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
