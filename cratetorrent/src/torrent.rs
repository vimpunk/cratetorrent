use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use futures::{
    select,
    stream::{Fuse, StreamExt},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
    task, time,
};

use crate::{
    disk::{DiskHandle, TorrentAlert, TorrentAlertReceiver},
    download::PieceDownload,
    error::*,
    peer::{self, PeerSession},
    piece_picker::PiecePicker,
    storage_info::StorageInfo,
    tracker::{Announce, Event, Tracker},
    Bitfield, PeerId, PieceIndex, Sha1Hash, TorrentId,
};

/// Represents a torrent upload or download.
///
/// This is the main entity responsible for the high-level management of
/// a torrent download or upload. It starts and stops connections with peers
/// ([`PeerSession`](crate::peer::PeerSession) instances) and stores metadata
/// about the torrent.
pub(crate) struct Torrent {
    /// The peers in this torrent.
    peers: Vec<Peer>,
    /// General status and information about the torrent.
    state: State,
    /// The handle to the disk IO task, used to issue commands on it. A copy of
    /// this handle is passed down to each peer session.
    disk: DiskHandle,
    /// The port on which we're receiving disk IO notifications of block write
    /// and piece completion.
    ///
    /// The channel has to be wrapped in a `stream::Fuse` so that we can
    /// `select!` on it in the torrent event loop.
    disk_alert_port: Fuse<TorrentAlertReceiver>,
    /// The trackers we can announce to.
    trackers: Vec<TrackerEntry>,
    /// The address on which torrent should listen for new peers.
    listen_addr: SocketAddr,
}

impl Torrent {
    /// Creates a new `Torrent` instance for downloading or seeding a torrent.
    ///
    /// # Important
    ///
    /// This constructor only initializes the torrent components but does not
    /// actually start it. See [`Self::start`].
    pub fn new(
        id: TorrentId,
        disk: DiskHandle,
        disk_alert_port: TorrentAlertReceiver,
        info_hash: Sha1Hash,
        storage_info: StorageInfo,
        have_pieces: Bitfield,
        trackers: Vec<Tracker>,
        client_id: PeerId,
        listen_addr: SocketAddr,
    ) -> Self {
        let piece_picker = PiecePicker::new(have_pieces);
        let status = State {
            context: Arc::new(TorrentContext {
                id,
                piece_picker: Arc::new(RwLock::new(piece_picker)),
                downloads: RwLock::new(HashMap::new()),
                info_hash,
                client_id,
                storage: storage_info,
                stats: TorrentStats::default(),
            }),
            start_time: None,
            run_duration: Duration::default(),
        };
        let disk_alert_port = disk_alert_port.fuse();

        let trackers =
            trackers.into_iter().map(|t| TrackerEntry::new(t)).collect();

        Self {
            peers: Vec::new(),
            state: status,
            disk,
            disk_alert_port,
            trackers,
            listen_addr,
        }
    }

    /// Starts the torrent and runs until an error is encountered.
    pub async fn start(&mut self, peers: &[SocketAddr]) -> Result<()> {
        log::info!("Starting torrent");

        // record the torrent starttime
        self.state.start_time = Some(Instant::now());

        // start all seed peer sessions, if any
        for addr in peers.iter().cloned() {
            let (session, chan) = PeerSession::new(
                Arc::clone(&self.state.context),
                self.disk.clone(),
                addr,
            );
            self.peers.push(Peer::start_outbound(session, chan))
        }

        // TODO: if the torrent is a seed, don't send the started event, just an
        // empty announce (verify this is the case)
        self.announce_to_trackers(Instant::now(), Some(Event::Started))
            .await?;

        let mut loop_timer = time::interval(Duration::from_secs(1)).fuse();
        let mut prev_instant = None;

        let mut listener = TcpListener::bind(&self.listen_addr).await?;
        let mut incoming = listener.incoming().fuse();

        // the torrent loop is triggered every second by the loop timer and by
        // disk IO events
        loop {
            select! {
                instant = loop_timer.select_next_some() => {
                    self.tick(&mut prev_instant, instant.into_std()).await?;
                }
                conn_result = incoming.select_next_some() => {
                    let socket = match conn_result {
                        Ok(socket) => socket,
                        Err(e) => {
                            log::info!("Error accepting peer connection: {}", e);
                            continue;
                        }
                    };
                    let addr = match socket.peer_addr() {
                        Ok(addr) => addr,
                        Err(e) => {
                            log::info!("Error getting socket address of peer: {}", e);
                            continue;
                        }
                    };
                    log::info!("New connection {:?}", addr);

                    // start inbound session
                    let (session, chan) = PeerSession::new(
                        Arc::clone(&self.state.context),
                        self.disk.clone(),
                        addr,
                    );
                    self.peers.push(Peer::start_inbound(socket, session, chan));
                }
                disk_alert = self.disk_alert_port.select_next_some() => {
                    let should_stop = self.handle_disk_alert(disk_alert).await?;
                    if should_stop {
                        // send shutdown command to all connected peers
                        for peer in self.peers.iter() {
                            if let Some(chan) = &peer.chan {
                                // we don't particularly care if we weren't successful
                                // in sending the command (for now)
                                chan.send(peer::Command::Shutdown).ok();
                            }
                        }

                        // tell trackers we're leaving
                        self.announce_to_trackers(Instant::now(), Some(Event::Stopped)).await?;

                        return Ok(());
                    }
                }
            }
        }
    }

    async fn tick(
        &mut self,
        prev_tick_time: &mut Option<Instant>,
        now: Instant,
    ) -> Result<()> {
        // calculate how long torrent has been running

        let elapsed_since_last_tick = prev_tick_time
            .or(self.state.start_time)
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or_default();
        self.state.run_duration += elapsed_since_last_tick;
        *prev_tick_time = Some(now);

        // check if we need to announce to some trackers
        let event = None;
        self.announce_to_trackers(now, event).await?;

        log::debug!("Info: elapsed: {} s", self.state.run_duration.as_secs());

        Ok(())
    }

    /// Chacks whether we need to announce to any trackers of if we need to request
    /// peers.
    async fn announce_to_trackers(
        &mut self,
        now: Instant,
        event: Option<Event>,
    ) -> Result<()> {
        // calculate transfer statistics in advance
        let uploaded = self
            .state
            .context
            .stats
            .uploaded_payload_count
            .load(Ordering::Relaxed);
        let downloaded = self
            .state
            .context
            .stats
            .downloaded_payload_count
            .load(Ordering::Relaxed);
        let left = self.state.context.storage.download_len - downloaded;

        /// The minimum number of peers we want to keep in torrent at all times.
        /// This will be configurable later.
        const MIN_PEER_COUNT: usize = 10;

        for tracker in self.trackers.iter_mut() {
            // Check if the torrent's peer count has fallen below the minimum.
            // But don't request new peers otherwise or if we're about to stop
            // torrent.
            let needed_peer_count = if self.peers.len() >= MIN_PEER_COUNT
                || event == Some(Event::Stopped)
            {
                None
            } else {
                Some(MIN_PEER_COUNT - self.peers.len())
            };

            // we can override the normal annoucne interval if we need peers or
            // if we have an event to announce
            if event.is_some()
                || (needed_peer_count.is_some() && tracker.can_announce(now))
                || tracker.should_announce(now)
            {
                let params = Announce {
                    tracker_id: tracker.id.clone(),
                    info_hash: self.state.context.info_hash.clone(),
                    peer_id: self.state.context.client_id.clone(),
                    port: self.listen_addr.port(),
                    peer_count: needed_peer_count,
                    uploaded,
                    downloaded,
                    left,
                    ip: None,
                    event,
                };
                // TODO: We probably don't want to block the torrent event loop
                // here waiting on the tracker response. Instead, poll the
                // future in the event loop select call.
                match tracker.client.announce(params).await {
                    Ok(resp) => {
                        log::info!(
                            "Announced to tracker {}, response: {:?}",
                            tracker.client,
                            resp
                        );
                        tracker.last_announce_time = Some(now);
                        if let Some(tracker_id) = resp.tracker_id {
                            tracker.id = Some(tracker_id);
                        }
                        if let Some(failure_reason) = resp.failure_reason {
                            log::warn!(
                                "Error contacting tracker {}: {}",
                                tracker.client,
                                failure_reason
                            );
                        }
                        if let Some(warning_message) = resp.warning_message {
                            log::warn!(
                                "Warning from tracker {}: {}",
                                tracker.client,
                                warning_message
                            );
                        }
                        if let Some(interval) = resp.interval {
                            log::info!(
                                "Tracker {} interval: {} s",
                                tracker.client,
                                interval.as_secs()
                            );
                            tracker.interval = Some(interval);
                        }
                        if let Some(min_interval) = resp.min_interval {
                            log::info!(
                                "Tracker {} min min_interval: {} s",
                                tracker.client,
                                min_interval.as_secs()
                            );
                            tracker.min_interval = Some(min_interval);
                        }

                        if let (Some(seeder_count), Some(leecher_count)) =
                            (resp.seeder_count, resp.leecher_count)
                        {
                            log::debug!(
                                "Torrent seeds: {} and leeches: {}",
                                seeder_count,
                                leecher_count
                            );
                        }

                        if !resp.peers.is_empty() {
                            log::debug!(
                                "Received peers from tracker {}: {:?}",
                                tracker.client,
                                resp.peers
                            );
                            // FIXME: only connect to a single peer for
                            // easier debugging purposes
                            if self.peers.is_empty() {
                                for addr in resp.peers.into_iter().take(1) {
                                    let (session, chan) = PeerSession::new(
                                        Arc::clone(&self.state.context),
                                        self.disk.clone(),
                                        addr,
                                    );
                                    self.peers.push(Peer::start_outbound(
                                        session, chan,
                                    ))
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "Error announcing to tracker {}: {}",
                            tracker.client,
                            e
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Handles the disk message and returns whether the message should cause
    /// the torrent to stop.
    async fn handle_disk_alert(
        &self,
        disk_alert: TorrentAlert,
    ) -> Result<bool> {
        match disk_alert {
            TorrentAlert::PieceCompletion(write_result) => {
                log::debug!("Disk write result {:?}", write_result);
                match write_result {
                    Ok(piece) => {
                        // if this write completed a piece, check torrent
                        // completion
                        if piece.is_valid {
                            // note that this piece picker is set as complete by
                            // peer sessions
                            let missing_piece_count = self
                                .state
                                .context
                                .piece_picker
                                .read()
                                .await
                                .count_missing_pieces();
                            log::info!(
                                "Finished piece {} download, valid: {}, left: {}",
                                piece.index,
                                piece.is_valid, missing_piece_count
                            );

                            // tell all peers that we got a new piece
                            for peer in self.peers.iter() {
                                if let Some(chan) = &peer.chan {
                                    chan.send(peer::Command::NewPiece(
                                        piece.index,
                                    ))?;
                                }
                            }

                            // if the torrent is fully downloaded, stop the
                            // download loop
                            if missing_piece_count == 0 {
                                // TODO: return a global alert here instead and
                                // let the engine decide whether to stop the
                                // torrent
                                log::info!(
                                    "Finished torrent download, exiting"
                                );
                                return Ok(true);
                            }
                        } else {
                            // TODO(https://github.com/mandreyel/cratetorrent/issues/61):
                            // Implement parole mode for the peers that sent
                            // corrupt data.
                            log::warn!("Received invalid piece, aborting");
                            return Ok(true);
                        }
                    }
                    Err(e) => {
                        // TODO: include details in the error as to which blocks
                        // failed to write
                        log::error!("Failed to write batch to disk: {}", e);
                    }
                }
            }
            TorrentAlert::ReadError { block_info, error } => {
                log::error!(
                    "Failed to read from disk {}: {}",
                    block_info,
                    error
                );
                // TODO: For now we just log for simplicity's sake, but in the
                // future we'll need error recovery mechanisms here.
                // For instance, it may be that the torrent file got moved while
                // the torrent was still seeding. In this case we'd need to stop
                // torrent and send an alert to the API consumer.
            }
        }

        Ok(false)
    }
}

/// A peer in the torrent.
struct Peer {
    /// The channel on which to communicate with the peer session.
    ///
    /// This is set when the session is started.
    chan: Option<peer::Sender>,
    /// The join handle to the peer session task.
    ///
    /// This is set when the session is started.
    // TODO(https://github.com/mandreyel/cratetorrent/issues/62): this will be
    // used once global stop command is implemented
    handle: Option<task::JoinHandle<Result<()>>>,
}

impl Peer {
    fn start_outbound(mut session: PeerSession, chan: peer::Sender) -> Self {
        let handle = task::spawn(async move { session.start_outbound().await });
        Self {
            chan: Some(chan),
            handle: Some(handle),
        }
    }

    fn start_inbound(
        socket: TcpStream,
        mut session: PeerSession,
        chan: peer::Sender,
    ) -> Self {
        let handle =
            task::spawn(async move { session.start_inbound(socket).await });
        Self {
            chan: Some(chan),
            handle: Some(handle),
        }
    }
}

/// Internal state information of torrent.
struct State {
    /// Information that is shared with peer sessions.
    context: Arc<TorrentContext>,
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

/// Information and methods shared with peer sessions in the torrent.
///
/// This type contains fields that need to be read or updated by peer sessions.
/// Fields expected to be mutated are thus secured for inter-task access with
/// various synchronization primitives.
pub(crate) struct TorrentContext {
    /// The torrent ID, unique in this engine.
    pub id: TorrentId,
    /// The info hash of the torrent, derived from its metainfo. This is used to
    /// identify the torrent with other peers and trackers.
    pub info_hash: Sha1Hash,
    /// The arbitrary client id, chosen by the user of this library. This is
    /// advertised to peers and trackers.
    pub client_id: PeerId,

    /// The piece picker picks the next most optimal piece to download and is
    /// shared by all peers in a torrent.
    pub piece_picker: Arc<RwLock<PiecePicker>>,
    /// These are the active piece downloads in which the peer sessions in this
    /// torrent are participating.
    ///
    /// They are stored and synchronized in this object to download a piece from
    /// multiple peers, which helps us to have fewer incomplete pieces.
    ///
    /// Peer sessions may be run on different threads, any of which may read and
    /// write to this map and to the pieces in the map. Thus we need a read
    /// write lock on both.
    // TODO: Benchmark whether using the nested locking approach isn't too slow.
    // For mvp it should do.
    pub downloads: RwLock<HashMap<PieceIndex, RwLock<PieceDownload>>>,

    /// Info about the torrent's storage (piece length, download length, etc).
    pub storage: StorageInfo,

    /// Statistics updated by peers of torrent, hence being in context.
    pub stats: TorrentStats,
}

/// Holds statistics related to a torrent.
///
/// This is shared with peer sessions so that they can update these stats in
/// real time. Therefore this type needs to be `Send + Sync`.
#[derive(Default)]
pub(crate) struct TorrentStats {
    /// Counts the total bytes sent during protocol chatter.
    pub downloaded_protocol_count: AtomicU64,
    /// Counts the total bytes received during protocol chatter.
    pub uploaded_protocol_count: AtomicU64,
    /// Counts the total downloaded block bytes.
    pub downloaded_payload_count: AtomicU64,
    /// Counts the total uploaded block bytes.
    pub uploaded_payload_count: AtomicU64,
}

/// Contains the tracker client as well as additional metadata about the
/// tracker.
struct TrackerEntry {
    client: Tracker,
    /// If a previous announce contained a tracker_id, it should be included in
    /// next announces. Therefore it is cached here.
    id: Option<String>,
    /// The last announce time is kept here so that we don't request too often.
    last_announce_time: Option<Instant>,
    /// The interval at which we should update the tracker of our progress.
    /// This is set after the first announce request.
    interval: Option<Duration>,
    /// The absolute minimum interval at which we can contact tracker.
    /// This is set after the first announce request.
    min_interval: Option<Duration>,
}

impl TrackerEntry {
    fn new(client: Tracker) -> Self {
        Self {
            client,
            id: None,
            last_announce_time: None,
            interval: None,
            min_interval: None,
        }
    }

    /// Determines whether we should announce to the tracker at the given time,
    /// based on when we last announced.
    ///
    /// Later this function should take into consideration the client's minimum
    /// announce frequency settings.
    fn should_announce(&self, t: Instant) -> bool {
        if let Some(last_announce_time) = self.last_announce_time {
            last_announce_time + self.interval.unwrap_or_default() > t
        } else {
            true
        }
    }

    /// Determines whether we're allowed to announce at the given time.
    ///
    /// We may need peers before the next step in the announce interval.
    /// However, we can't do this too often, so we need to check our last
    /// announce time first.
    fn can_announce(&self, t: Instant) -> bool {
        if let Some(last_announce_time) = self.last_announce_time {
            last_announce_time + self.min_interval.unwrap_or_default() >= t
        } else {
            true
        }
    }
}
