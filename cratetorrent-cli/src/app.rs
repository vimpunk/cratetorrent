use std::collections::HashMap;
use std::{fs, path::PathBuf, time::Duration};

use cratetorrent::{
    alert::AlertReceiver,
    conf::Conf,
    engine::{EngineHandle, TorrentParams},
    metainfo::Metainfo,
    storage_info::FsStructure,
    torrent::stats::{PieceStats, ThroughputStats, TorrentStats},
    TorrentId,
};
use futures::stream::{Fuse, StreamExt};

use crate::{Args, Result};

/// Holds the application state.
pub struct App {
    pub engine: EngineHandle,
    pub alert_rx: Fuse<AlertReceiver>,
    pub torrents: HashMap<TorrentId, Torrent>,
}

impl App {
    pub fn new(download_dir: PathBuf) -> Result<Self> {
        // start engine
        let conf = Conf::new(download_dir);
        let (engine, alert_rx) = cratetorrent::engine::spawn(conf)?;
        let alert_rx = alert_rx.fuse();

        Ok(Self {
            engine,
            alert_rx,
            torrents: HashMap::new(),
        })
    }

    pub fn create_torrent(&mut self, args: Args) -> Result<()> {
        // read in torrent metainfo
        let metainfo = fs::read(&args.metainfo)?;
        let metainfo = Metainfo::from_bytes(&metainfo)?;
        let info_hash = hex::encode(&metainfo.info_hash);
        let piece_count = metainfo.piece_count();
        let download_len = metainfo.structure.download_len();

        // create torrent
        let torrent_id = self.engine.create_torrent(TorrentParams {
            metainfo: metainfo.clone(),
            listen_addr: args.listen,
            mode: args.mode,
            conf: None,
        })?;

        let torrent = Torrent {
            name: metainfo.name,
            info_hash,
            piece_len: metainfo.piece_len,
            download_len,
            fs: metainfo.structure,

            run_duration: Default::default(),
            pieces: PieceStats {
                total: piece_count,
                ..Default::default()
            },
            peer_count: Default::default(),
            downloaded_payload_stats: Default::default(),
            wasted_payload_count: Default::default(),
            uploaded_payload_stats: Default::default(),
            downloaded_protocol_stats: Default::default(),
            uploaded_protocol_stats: Default::default(),
        };
        self.torrents.insert(torrent_id, torrent);

        Ok(())
    }

    pub fn update_torrent_state(
        &mut self,
        torrent_id: TorrentId,
        stats: TorrentStats,
    ) {
        if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
            torrent.run_duration = stats.run_duration;
            torrent.pieces = stats.pieces;
            torrent.peer_count = stats.peer_count;

            const HISTORY_LIMIT: usize = 100;
            for (history, update) in [
                (
                    &mut torrent.downloaded_payload_stats,
                    &stats.downloaded_payload_stats,
                ),
                (
                    &mut torrent.uploaded_payload_stats,
                    &stats.uploaded_payload_stats,
                ),
                (
                    &mut torrent.downloaded_protocol_stats,
                    &stats.downloaded_protocol_stats,
                ),
                (
                    &mut torrent.uploaded_protocol_stats,
                    &stats.uploaded_protocol_stats,
                ),
            ]
            .iter_mut()
            {
                history.update(update, HISTORY_LIMIT);
            }
        }
    }
}

/// Holds state about a single torrent.
pub struct Torrent {
    // static info
    pub name: String,
    pub info_hash: String,
    pub piece_len: u32,
    pub download_len: u64,
    pub fs: FsStructure,
    // TODO
    // total download size

    // dynamic info
    pub run_duration: Duration,
    pub pieces: PieceStats,
    pub peer_count: usize,

    pub downloaded_payload_stats: ThroughputHistory,
    pub wasted_payload_count: u64,
    pub uploaded_payload_stats: ThroughputHistory,
    pub downloaded_protocol_stats: ThroughputHistory,
    pub uploaded_protocol_stats: ThroughputHistory,
    //
    // TODO:
    // downloaded
    // list of files with their sizes (map fs to vec<files>)
}

#[derive(Default)]
pub struct ThroughputHistory {
    pub peak: u64,
    pub total: u64,
    pub rate_history: Vec<u64>,
}

impl ThroughputHistory {
    pub fn update(&mut self, stats: &ThroughputStats, limit: usize) {
        if self.rate_history.len() >= limit {
            // pop the first element
            // TODO: make this more optimal
            self.rate_history.drain(0..1);
        }
        self.rate_history.push(stats.rate);
        self.peak = stats.peak;
        self.total = stats.total;
    }
}
