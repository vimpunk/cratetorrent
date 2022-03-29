//! This module defines types used to configure the engine and its parts.

use std::{path::PathBuf, time::Duration};

use crate::PeerId;

/// The default cratetorrent client id.
pub const CRATETORRENT_CLIENT_ID: &PeerId = b"cbt-0000000000000000";

/// The global configuration for the torrent engine and all its parts.
#[derive(Clone, Debug)]
pub struct Conf {
    pub engine: EngineConf,
    pub torrent: TorrentConf,
}

impl Conf {
    /// Returns the torrent configuration with reasonable defaults, except for
    /// the download directory, as it is not sensible to guess that for the
    /// user. It uses the default cratetorrent client id,
    /// [`CRATETORRENT_CLIENT_ID`].
    pub fn new(download_dir: impl Into<PathBuf>) -> Self {
        Self {
            engine: EngineConf {
                client_id: *CRATETORRENT_CLIENT_ID,
                download_dir: download_dir.into(),
            },
            torrent: TorrentConf::default(),
        }
    }
}

/// Configuration related to the engine itself.
#[derive(Clone, Debug)]
pub struct EngineConf {
    /// The ID of the client to announce to trackers and other peers.
    pub client_id: PeerId,
    /// The directory in which a torrent's files are placed upon download and
    /// from which they are seeded.
    pub download_dir: PathBuf,
}

/// Configuration for a torrent.
///
/// The engine will have a default instance of this applied to all torrents by
/// default, but individual torrents may override this configuration.
#[derive(Clone, Debug)]
pub struct TorrentConf {
    /// The minimum number of peers we want to keep in torrent at all times.
    /// This will be configurable later.
    pub min_requested_peer_count: usize,

    /// The max number of connected peers the torrent should have.
    pub max_connected_peer_count: usize,

    /// If the tracker doesn't provide a minimum announce interval, we default
    /// to announcing every 30 seconds.
    pub announce_interval: Duration,

    /// After this many attempts, the torrent stops announcing to a tracker.
    pub tracker_error_threshold: usize,

    /// Timeout duration after error threshold is reached.
    pub tracker_error_timeout: Duration,

    /// Specifies which optional alerts to send, besides the default periodic
    /// stats update.
    pub alerts: TorrentAlertConf,
}

/// Configuration of a torrent's optional alerts.
///
/// By default, all optional alerts are turned off. This is because some of
/// these alerts may have overhead that shouldn't be paid when the alerts are
/// not used.
#[derive(Clone, Debug, Default)]
pub struct TorrentAlertConf {
    /// Receive the pieces that were completed each round.
    ///
    /// This has minor overhead and so it may be enabled. For full optimization,
    /// however, it is only enabled when either the pieces or individual file
    /// completions are needed.
    pub completed_pieces: bool,
    /// Receive aggregate statistics about the torrent's peers.
    ///
    /// This may be relatively expensive. It is suggested to only turn it on
    /// when it is specifically needed, e.g. when the UI is showing the peers of
    /// a torrent.
    pub peers: bool,
}

impl Default for TorrentConf {
    fn default() -> Self {
        Self {
            // We always request at least 10 peers as anything less is a waste
            // of network round trip and it allows us to buffer up a bit more
            // than needed.
            min_requested_peer_count: 10,
            // This value is mostly picked for performance while keeping in mind
            // not to overwhelm the host.
            max_connected_peer_count: 50,
            // TODO: needs testing
            announce_interval: Duration::from_secs(60 * 60),
            // TODO: needs testing
            tracker_error_threshold: 15,
            // TODO: needs testing
            tracker_error_timeout: Duration::from_secs(60 * 60),
            alerts: Default::default(),
        }
    }
}
