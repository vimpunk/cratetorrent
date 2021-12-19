use std::collections::HashMap;
use std::{fs, path::PathBuf, time::Duration};

use cratetorrent::{
    alert::AlertReceiver,
    conf::{Conf, TorrentAlertConf, TorrentConf},
    engine::{EngineHandle, Mode, TorrentParams},
    metainfo::Metainfo,
    storage_info::StorageInfo,
    torrent::stats::{Channel, Peers, PieceStats, Thruput, TorrentStats},
    FileInfo, TorrentId,
};

use crate::{Args, Result};

/// Holds the application state.
pub struct App {
    pub download_dir: PathBuf,
    pub engine: EngineHandle,
    pub alert_rx: AlertReceiver,
    pub torrents: HashMap<TorrentId, Torrent>,
}

impl App {
    pub fn new(download_dir: PathBuf) -> Result<Self> {
        // start engine
        let conf = Conf::new(download_dir.clone());
        let (engine, alert_rx) = cratetorrent::engine::spawn(conf)?;

        Ok(Self {
            download_dir,
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
        let download_len = metainfo.download_len();
        let is_seed = matches!(args.mode, Mode::Seed);

        let storage = StorageInfo::new(&metainfo, self.download_dir.clone());
        let files = storage
            .files
            .iter()
            .map(|f| FileStats {
                info: f.clone(),
                complete: if is_seed { f.len } else { 0 },
            })
            .collect();

        let pieces = if is_seed {
            PieceStats {
                total: piece_count,
                complete: piece_count,
                ..Default::default()
            }
        } else {
            PieceStats {
                total: piece_count,
                latest_completed: Some(Vec::new()),
                ..Default::default()
            }
        };

        // create torrent
        let torrent_id = self.engine.create_torrent(TorrentParams {
            metainfo: metainfo.clone(),
            listen_addr: args.listen,
            mode: args.mode,
            conf: Some(TorrentConf {
                alerts: TorrentAlertConf {
                    completed_pieces: true,
                    peers: true,
                },
                ..Default::default()
            }),
        })?;

        let torrent = Torrent {
            name: metainfo.name,
            info_hash,
            piece_len: metainfo.piece_len,
            download_len,
            storage,

            run_duration: Default::default(),
            pieces,
            files,
            peers: Default::default(),
            protocol: Default::default(),
            payload: Default::default(),
            wasted_payload_count: Default::default(),
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
            // update file completion by checking which pieces were downloaded
            // this round
            // TODO: clean this up or possibly move into cratetorrent as
            // a utility function
            // TODO: consider letting tradetorrent send file completion progress
            // since if a client is not listening continuously for completed
            // pieces they won't be able to reconsruct this
            if let Some(pieces) = &stats.pieces.latest_completed {
                // for each piece, check which
                for piece in pieces.iter().cloned() {
                    let piece_len = torrent.storage.piece_len(piece);
                    let mut torrent_piece_offset =
                        torrent.storage.torrent_piece_offset(piece);
                    let mut consumed = 0;

                    let file_range =
                        torrent.storage.files_intersecting_piece(piece);
                    let files = &mut torrent.files[file_range];
                    for file in files.iter_mut() {
                        let remaining_piece_len = piece_len as u64 - consumed;
                        let file_slice = file.info.get_slice(
                            torrent_piece_offset,
                            remaining_piece_len,
                        );
                        torrent_piece_offset += file_slice.len;
                        consumed += file_slice.len;
                        file.complete += file_slice.len;
                        debug_assert!(
                            file.complete <= file.info.len,
                            "cannot have downloaded more than file length"
                        );
                    }
                }
            }

            // update piece download stats, but take care not to overwrite our
            // piece history, which is needed to produce a continuous event list
            let latest_completed_pieces =
                if let (Some(mut existing), Some(new)) = (
                    torrent.pieces.latest_completed.take(),
                    stats.pieces.latest_completed,
                ) {
                    existing.extend(new.into_iter());
                    if existing.len() > 100 {
                        let overflow = existing.len() - 100;
                        existing.drain(0..overflow);
                    }
                    Some(existing)
                } else {
                    None
                };

            torrent.pieces = PieceStats {
                latest_completed: latest_completed_pieces,
                ..stats.pieces
            };
            torrent.peers = stats.peers;
            let is_seed = torrent.is_seed();
            if let Peers::Full(peers) = &mut torrent.peers {
                // sort peers by thruput rate in descending order: by upload
                // rate if torrent is seeding, by download rate otherwise
                if is_seed {
                    peers.sort_by(|a, b| {
                        b.thruput
                            .payload
                            .up
                            .rate
                            .cmp(&a.thruput.payload.up.rate)
                    });
                } else {
                    peers.sort_by(|a, b| {
                        b.thruput
                            .payload
                            .down
                            .rate
                            .cmp(&a.thruput.payload.down.rate)
                    });
                }
            }
            torrent.run_duration = stats.run_duration;

            const HISTORY_LIMIT: usize = 300;
            for (history, curr) in [
                (&mut torrent.payload, &stats.thruput.payload),
                (&mut torrent.protocol, &stats.thruput.protocol),
            ]
            .iter_mut()
            {
                history.update(curr, HISTORY_LIMIT);
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
    pub storage: StorageInfo,

    // dynamic info
    pub run_duration: Duration,
    pub pieces: PieceStats,
    pub peers: Peers,

    pub files: Vec<FileStats>,

    pub protocol: ChannelHistory,
    pub payload: ChannelHistory,
    pub wasted_payload_count: u64,
}

impl Torrent {
    fn is_seed(&self) -> bool {
        self.pieces.is_seed()
    }
}

#[derive(Default)]
pub struct ChannelHistory {
    pub down: ThruputHistory,
    pub up: ThruputHistory,
}

impl ChannelHistory {
    pub fn update(&mut self, c: &Channel, limit: usize) {
        self.down.update(&c.down, limit);
        self.up.update(&c.up, limit);
    }
}

#[derive(Default)]
pub struct ThruputHistory {
    pub peak: u64,
    pub total: u64,
    /// Historical data of download rates.
    pub rates: Vec<u64>,
}

impl ThruputHistory {
    pub fn update(&mut self, t: &Thruput, limit: usize) {
        if self.rates.len() >= limit {
            // pop the first element
            // TODO: make this more optimal
            self.rates.drain(0..1);
        }
        self.rates.push(t.rate);
        self.peak = t.peak;
        self.total = t.total;
    }

    pub fn rate(&self) -> u64 {
        self.rates.iter().rev().next().cloned().unwrap_or_default()
    }
}

pub struct FileStats {
    pub info: FileInfo,
    pub complete: u64,
}
