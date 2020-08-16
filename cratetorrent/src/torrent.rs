use {
    futures::{
        select,
        stream::{Fuse, StreamExt},
    },
    std::{
        net::SocketAddr,
        sync::Arc,
        time::{Duration, Instant},
    },
    tokio::{sync::RwLock, task, time},
};

use crate::{
    disk::{DiskHandle, TorrentAlert, TorrentAlertReceiver},
    error::*,
    peer::{self, PeerSession},
    piece_picker::PiecePicker,
    storage_info::StorageInfo,
    PeerId, Sha1Hash, TorrentId,
};

pub(crate) struct Torrent {
    /// The only connection the torrent is connecting to.
    seeds: Vec<Peer>,
    /// General status and information about the torrent.
    status: Status,
    /// This is passed to peer and tracks the availability of our pieces as well
    /// as pieces in the torrent swarm (more relevant when more peers are
    /// added), and using this knowledge which piece to pick next.
    piece_picker: Arc<RwLock<PiecePicker>>,
    /// The handle to the disk IO task, used to issue commands on it. A copy of
    /// this handle is passed down to each peer session.
    disk: DiskHandle,
    /// The port on which we're receiving disk IO notifications of block write
    /// and piece completion.
    ///
    /// The channel has to be wrapped in a `stream::Fuse` so that we can
    /// `select!` on it in the torrent event loop.
    disk_alert_port: Fuse<TorrentAlertReceiver>,
}

impl Torrent {
    /// Creates a new `Torrent` instance, initializing its core components but
    /// not starting it.
    pub fn new(
        id: TorrentId,
        disk: DiskHandle,
        disk_alert_port: TorrentAlertReceiver,
        info_hash: Sha1Hash,
        storage_info: StorageInfo,
        client_id: PeerId,
        seeds: &[SocketAddr],
    ) -> Result<Self> {
        log::trace!("Creating torrent {} with seeds: {:?}", id, seeds);

        let piece_count = storage_info.piece_count;
        let status = Status {
            shared: Arc::new(SharedStatus {
                id,
                info_hash,
                client_id,
                storage: storage_info,
            }),
            start_time: None,
            run_duration: Duration::default(),
        };

        let piece_picker = PiecePicker::new(piece_count);
        let piece_picker = Arc::new(RwLock::new(piece_picker));

        let disk_alert_port = disk_alert_port.fuse();

        Ok(Self {
            seeds: seeds
                .iter()
                .map(|&addr| Peer {
                    addr,
                    chan: None,
                    handle: None,
                })
                .collect(),
            status,
            piece_picker,
            disk,
            disk_alert_port,
        })
    }

    /// Starts the torrent and returns normally if the download is complete, or
    /// aborts if an error is encountered.
    pub async fn start(&mut self) -> Result<()> {
        log::info!("Starting torrent");

        // record the torrent starttime
        self.status.start_time = Some(Instant::now());

        // start all seed peer sessions
        for peer in self.seeds.iter_mut() {
            let (mut session, chan) = PeerSession::outbound(
                Arc::clone(&self.status.shared),
                Arc::clone(&self.piece_picker),
                self.disk.clone(),
                peer.addr,
            );
            let handle = task::spawn(async move { session.start().await });
            peer.chan = Some(chan);
            peer.handle = Some(handle);
        }

        let mut loop_timer = time::interval(Duration::from_secs(1)).fuse();
        let mut prev_instant = None;

        // the torrent loop is triggered every second by the loop timer and by
        // disk IO events
        loop {
            select! {
                instant = loop_timer.select_next_some() => {
                    // calculate how long torrent has been running
                    //
                    // only deal with std time types
                    let instant = instant.into_std();
                    let elapsed = if let Some(prev_instant) = prev_instant {
                        instant.saturating_duration_since(prev_instant)
                    } else if let Some(start_time) = self.status.start_time {
                        instant.saturating_duration_since(start_time)
                    } else {
                        Duration::default()
                    };
                    self.status.run_duration += elapsed;
                    prev_instant = Some(instant);

                    log::debug!(
                        "Torrent running for {}s",
                        self.status.run_duration.as_secs()
                    );
                }
                disk_alert = self.disk_alert_port.select_next_some() => {
                    let should_stop = self.handle_disk_alert(disk_alert).await?;
                    if should_stop {
                        // we don't particularly care if we weren't successful
                        // in sending the command (for now)
                        for peer in self.seeds.iter() {
                            if let Some(chan) = &peer.chan {
                                chan.send(peer::Command::Shutdown).ok();
                            }
                        }
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn handle_disk_alert(
        &self,
        disk_alert: TorrentAlert,
    ) -> Result<bool> {
        match disk_alert {
            TorrentAlert::BatchWrite(write_result) => {
                log::debug!("Disk write result {:?}", write_result);

                match write_result {
                    Ok(batch) => {
                        // if this write completed a piece, check torrent
                        // completion
                        if let Some(is_piece_valid) = batch.is_piece_valid {
                            if is_piece_valid {
                                log::info!(
                                    "Finished piece download, valid? {}",
                                    is_piece_valid
                                );
                                let missing_piece_count = self
                                    .piece_picker
                                    .read()
                                    .await
                                    .count_missing_pieces();
                                log::debug!(
                                    "Piece(s) left: {}",
                                    missing_piece_count
                                );

                                // if the torrent is fully downloaded, stop the
                                // download loop
                                if missing_piece_count == 0 {
                                    log::info!(
                                        "Finished torrent download, exiting"
                                    );
                                    return Ok(true);
                                }
                            } else {
                                log::warn!("Received invalid piece, aborting");
                                return Ok(true);
                            }
                        }
                    }
                    Err(e) => {
                        // TODO: include details in the error as to which blocks
                        // failed to write
                        log::warn!("Failed to write batch to disk: {}", e);
                    }
                }
            }
        }

        Ok(false)
    }
}

/// A peer in the torrent.
struct Peer {
    /// The address of a single peer this torrent donwloads the file from. This
    /// peer has to be a seed as currently we only support downloading and no
    /// seeding.
    addr: SocketAddr,
    /// The channel on which to communicate with the peer session.
    ///
    /// This is set when the session is started.
    chan: Option<peer::Sender>,
    /// The join handle to the peer session task.
    ///
    /// This is set when the session is started.
    handle: Option<task::JoinHandle<Result<()>>>,
}

/// Status information of a torrent.
struct Status {
    /// Information that is shared with peer sessions.
    shared: Arc<SharedStatus>,
    /// The time the torrent was first started.
    start_time: Option<Instant>,
    /// The total time the torrent has been running.
    ///
    /// This is a separate field as `Instant::now() - start_time` cannot be
    /// relied upon due to the fact that it is possible to pause a torrent, in
    /// which case we don't want to record the run time.
    // TODO: pausing a torrent is not actually at this point, but this is done
    // in expectation of that feature
    run_duration: Duration,
}

/// Information shared with peer sessions.
///
/// This type contains fields that need to be read or updated by peer sessions.
/// Fields expected to be mutated are thus secured for inter-task access with
/// various synchronization primitives.
#[derive(Debug)]
pub(crate) struct SharedStatus {
    /// The torrent ID, unique in this engine.
    pub id: TorrentId,
    /// The info hash of the torrent, derived from its metainfo. This is used to
    /// identify the torrent with other peers and trackers.
    pub info_hash: Sha1Hash,
    /// The arbitrary client id, chosen by the user of this library. This is
    /// advertised to peers and trackers.
    pub client_id: PeerId,
    /// Info about the torrent's storage (piece length, download length, etc).
    pub storage: StorageInfo,
}
