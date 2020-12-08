use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{
    select,
    stream::{Fuse, StreamExt},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        RwLock,
    },
    task, time,
};

use crate::{
    counter::Counter,
    disk::{DiskHandle, ReadError, WriteError},
    download::PieceDownload,
    error::*,
    peer::{self, ConnectionState, PeerSession, SessionInfo, SessionState},
    piece_picker::PiecePicker,
    storage_info::StorageInfo,
    tracker::{Announce, Event, Tracker},
    Bitfield, BlockInfo, PeerId, PieceIndex, Sha1Hash, Side, TorrentId,
};

/// The channel for communicating with torrent.
pub(crate) type Sender = UnboundedSender<Message>;

/// The type of channel on which a torrent can listen for block write
/// completions.
pub(crate) type Receiver = UnboundedReceiver<Message>;

/// The types of messages that the torrent can receive from other parts of the
/// engine.
#[derive(Debug)]
pub(crate) enum Message {
    /// Sent when some blocks were written to disk or an error ocurred while
    /// writing.
    PieceCompletion(Result<PieceCompletion, WriteError>),
    /// There was an error reading a block.
    ReadError {
        block_info: BlockInfo,
        error: ReadError,
    },
    /// Peer sessions periodically send this message when they have a state
    /// change.
    PeerState { addr: SocketAddr, info: SessionInfo },
}

/// The type returned on completing a piece.
#[derive(Debug)]
pub(crate) struct PieceCompletion {
    /// The index of the piece.
    pub index: PieceIndex,
    /// Whether the piece is valid.
    ///
    /// If the piece is invalid, it is not written to disk.
    pub is_valid: bool,
}

/// Represents a torrent upload or download.
///
/// This is the main entity responsible for the high-level management of
/// a torrent download or upload. It starts and stops connections with peers
/// ([`PeerSession`](crate::peer::PeerSession) instances) and stores metadata
/// about the torrent.
pub(crate) struct Torrent {
    /// The peers in this torrent.
    peers: HashMap<SocketAddr, Peer>,
    /// The peers returned by tracker to which we can connect.
    available_peers: Vec<SocketAddr>,
    /// Information that is shared with peer sessions.
    ctx: Arc<TorrentContext>,
    /// The handle to the disk IO task, used to issue commands on it. A copy of
    /// this handle is passed down to each peer session.
    disk: DiskHandle,
    /// The port on which other entities in the engine send this torrent
    /// messages.
    ///
    /// The channel has to be wrapped in a `stream::Fuse` so that we can
    /// `select!` on it in the torrent event loop.
    port: Fuse<Receiver>,
    /// The trackers we can announce to.
    trackers: Vec<TrackerEntry>,

    /// The address on which torrent should listen for new peers.
    listen_addr: SocketAddr,

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

    /// Counts the total downloaded block bytes in torrent.
    downloaded_payload_counter: Counter,
    /// Counts the total uploaded block bytes in torrent.
    uploaded_payload_counter: Counter,

    /// Counts the total bytes sent during protocol chatter in torrent.
    downloaded_protocol_counter: Counter,
    /// Counts the total bytes received during protocol chatter in torrent.
    uploaded_protocol_counter: Counter,
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
        chan: Sender,
        port: Receiver,
        info_hash: Sha1Hash,
        storage_info: StorageInfo,
        have_pieces: Bitfield,
        trackers: Vec<Tracker>,
        client_id: PeerId,
        listen_addr: SocketAddr,
    ) -> Self {
        let piece_picker = PiecePicker::new(have_pieces);
        let port = port.fuse();
        let trackers =
            trackers.into_iter().map(|t| TrackerEntry::new(t)).collect();

        Self {
            peers: HashMap::new(),
            available_peers: Vec::new(),
            disk,
            ctx: Arc::new(TorrentContext {
                id,
                chan,
                piece_picker: Arc::new(RwLock::new(piece_picker)),
                downloads: RwLock::new(HashMap::new()),
                info_hash,
                client_id,
                storage: storage_info,
            }),
            start_time: None,
            run_duration: Duration::default(),
            port,
            trackers,

            downloaded_payload_counter: Default::default(),
            uploaded_payload_counter: Default::default(),
            downloaded_protocol_counter: Default::default(),
            uploaded_protocol_counter: Default::default(),

            listen_addr,
        }
    }

    /// Starts the torrent and runs until an error is encountered.
    pub async fn start(&mut self, peers: &[SocketAddr]) -> Result<()> {
        log::info!("Starting torrent");

        self.available_peers.extend_from_slice(peers);

        // record the torrent starttime
        self.start_time = Some(Instant::now());

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
                peer_conn_result = incoming.select_next_some() => {
                    let socket = match peer_conn_result {
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
                        Arc::clone(&self.ctx),
                        self.disk.clone(),
                        addr,
                    );
                    self.peers.insert(addr, Peer::start_inbound(socket, session, chan));
                }
                msg = self.port.select_next_some() => {
                    let should_stop = self.handle_msg(msg).await?;
                    if should_stop {
                        // send shutdown command to all connected peers
                        for peer in self.peers.values() {
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
            .or(self.start_time)
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or_default();
        self.run_duration += elapsed_since_last_tick;
        *prev_tick_time = Some(now);

        // check if we can connect some peers
        // NOTE: do this before announcing as we don't want to block new
        // connections with the potentially long running announce requests
        self.connect_peers();

        // check if we need to announce to some trackers
        let event = None;
        self.announce_to_trackers(now, event).await?;

        log::debug!(
            "Info: \
            elapsed {} s, \
            download: {} b/s (peak: {} b/s, total: {} b) \
            upload: {} b/s (peak: {} b/s, total: {} b)",
            self.run_duration.as_secs(),
            self.downloaded_payload_counter.avg(),
            self.downloaded_payload_counter.peak(),
            self.downloaded_payload_counter.total(),
            self.uploaded_payload_counter.avg(),
            self.uploaded_payload_counter.peak(),
            self.uploaded_payload_counter.total(),
        );

        for counter in [
            &mut self.downloaded_payload_counter,
            &mut self.uploaded_payload_counter,
            &mut self.downloaded_protocol_counter,
            &mut self.uploaded_protocol_counter,
        ]
        .iter_mut()
        {
            counter.reset();
        }

        Ok(())
    }

    /// Attempts to connect available peers, if we have any.
    fn connect_peers(&mut self) {
        let connect_count = MAX_CONNECTED_PEER_COUNT
            .saturating_sub(self.peers.len())
            .min(self.available_peers.len());
        if connect_count == 0 {
            log::trace!("Cannot connect to peers");
            return;
        }

        log::debug!("Connecting {} peer(s)", connect_count);
        for addr in self.available_peers.drain(0..connect_count) {
            log::info!("Connecting to peer {}", addr);
            let (session, chan) = PeerSession::new(
                Arc::clone(&self.ctx),
                self.disk.clone(),
                addr,
            );
            self.peers.insert(addr, Peer::start_outbound(session, chan));
        }
    }

    /// Chacks whether we need to announce to any trackers of if we need to request
    /// peers.
    async fn announce_to_trackers(
        &mut self,
        now: Instant,
        event: Option<Event>,
    ) -> Result<()> {
        // calculate transfer statistics in advance
        let uploaded = self.uploaded_payload_counter.total();
        let downloaded = self.downloaded_payload_counter.total();
        let left = self.ctx.storage.download_len - downloaded;

        // skip trackers that errored too often
        // TODO: introduce a retry timeout
        for tracker in self
            .trackers
            .iter_mut()
            .filter(|t| t.error_count < TRACKER_ERROR_THRESHOLD)
        {
            // Check if the torrent's peer count has fallen below the minimum.
            // But don't request new peers otherwise or if we're about to stop
            // torrent.
            let peer_count = self.peers.len() + self.available_peers.len();
            let needed_peer_count = if peer_count >= MIN_REQUESTED_PEER_COUNT
                || event == Some(Event::Stopped)
            {
                None
            } else {
                debug_assert!(MAX_CONNECTED_PEER_COUNT >= peer_count);
                let needed = MAX_CONNECTED_PEER_COUNT - peer_count;
                // Download at least this many peers, even if we don't need as
                // much. This is because later we may be able to connect to more
                // peers and in that case we don't want to wait till the next
                // tracker request.
                Some(MIN_REQUESTED_PEER_COUNT.min(needed))
            };

            // we can override the normal annoucne interval if we need peers or
            // if we have an event to announce
            if event.is_some()
                || (needed_peer_count > Some(0) && tracker.can_announce(now))
                || tracker.should_announce(now)
            {
                let params = Announce {
                    tracker_id: tracker.id.clone(),
                    info_hash: self.ctx.info_hash.clone(),
                    peer_id: self.ctx.client_id.clone(),
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
                // future in the event loop select call, or spawn the tracker
                // announce on a separate task and return the result as
                // an mpsc message.
                match tracker.client.announce(params).await {
                    Ok(resp) => {
                        log::info!(
                            "Announced to tracker {}, response: {:?}",
                            tracker.client,
                            resp
                        );
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
                            self.available_peers.extend(resp.peers.into_iter());
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "Error announcing to tracker {}: {}",
                            tracker.client,
                            e
                        );
                        tracker.error_count += 1;
                    }
                }
                tracker.last_announce_time = Some(now);
            }
        }

        Ok(())
    }

    /// Handles a message from another task in the engine.
    ///
    /// The message may result in the torrent scheduled to be shut down. This
    /// is indicated by the result value: if it's true, the torrent event loop
    /// exits.
    async fn handle_msg(&mut self, msg: Message) -> Result<bool> {
        match msg {
            Message::PeerState { addr, info } => {
                if let Some(peer) = self.peers.get_mut(&addr) {
                    log::debug!("Updating peer {} state", addr);
                    peer.state = info.state;
                    peer.state = info.state;
                    self.downloaded_payload_counter +=
                        info.throughput.downloaded_payload_count;
                    self.uploaded_payload_counter +=
                        info.throughput.uploaded_payload_count;
                    self.downloaded_protocol_counter +=
                        info.throughput.downloaded_protocol_count;
                    self.uploaded_protocol_counter +=
                        info.throughput.uploaded_protocol_count;

                    // if we disconnected peer, remove it
                    if peer.state.connection == ConnectionState::Disconnected {
                        self.peers.remove(&addr);
                    }
                } else {
                    log::debug!("Tried updating non-existent peer {}", addr);
                }
            }
            Message::PieceCompletion(write_result) => {
                log::debug!("Disk write result {:?}", write_result);
                match write_result {
                    Ok(piece) => {
                        // if this write completed a piece, check torrent
                        // completion
                        if piece.is_valid {
                            // note that this piece picker is set as complete by
                            // peer sessions
                            let missing_piece_count = self
                                .ctx
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
                            for peer in self.peers.values() {
                                if let Some(chan) = &peer.chan {
                                    // this message may be sent after the peer
                                    // session had already stopped but before
                                    // the torrent tick ran and got a chance to
                                    // reap the dead session
                                    chan.send(peer::Command::NewPiece(
                                        piece.index,
                                    ))
                                    .ok();
                                }
                            }

                            // if the torrent is fully downloaded, stop the
                            // download loop
                            if missing_piece_count == 0 {
                                // TODO: return a global alert here instead and
                                // let the user decide whether to stop the
                                // torrent
                                log::info!(
                                    "Finished torrent download, exiting. \
                                    Peak download rate: {} b/s",
                                    self.downloaded_payload_counter.peak(),
                                );

                                // tell trackers we've finished
                                self.announce_to_trackers(
                                    Instant::now(),
                                    Some(Event::Completed),
                                )
                                .await?;

                                return Ok(true);
                            }
                        } else {
                            // TODO(https://github.com/mandreyel/cratetorrent/issues/61):
                            // Implement parole mode for the peers that sent
                            // corrupt data.
                            log::warn!(
                                "Received invalid piece {}",
                                piece.index
                            );
                        }
                    }
                    Err(e) => {
                        // TODO: include details in the error as to which blocks
                        // failed to write
                        log::error!("Failed to write batch to disk: {}", e);
                    }
                }
            }
            Message::ReadError { block_info, error } => {
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

/// A peer in the torrent. Contains additional metadata needed by torrent to
/// manage the peer.
struct Peer {
    /// The channel on which to communicate with the peer session.
    ///
    /// This is set when the session is started.
    chan: Option<peer::Sender>,
    /// Cached information about the session state. Updated every time peer
    /// updates us.
    state: SessionState,
    /// Whether the peer is a seed or a leech.
    side: Side,
}

impl Peer {
    fn start_outbound(mut session: PeerSession, chan: peer::Sender) -> Self {
        task::spawn(async move { session.start_outbound().await });
        Self {
            chan: Some(chan),
            state: SessionState {
                connection: ConnectionState::Connecting,
                ..Default::default()
            },
            side: Default::default(),
        }
    }

    fn start_inbound(
        socket: TcpStream,
        mut session: PeerSession,
        chan: peer::Sender,
    ) -> Self {
        task::spawn(async move { session.start_inbound(socket).await });
        Self {
            chan: Some(chan),
            state: SessionState {
                connection: ConnectionState::Handshaking,
                ..Default::default()
            },
            side: Default::default(),
        }
    }
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

    /// A copy of the torrent channel sender. This is not used by torrent iself,
    /// but by the peer session tasks to which an arc copy of this torrent
    /// context is given.
    pub chan: Sender,

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
    /// Each time we fail to requet from tracker, this counter is incremented.
    /// If it fails too often, we stop requesting from tracker.
    error_count: usize,
}

impl TrackerEntry {
    fn new(client: Tracker) -> Self {
        Self {
            client,
            id: None,
            last_announce_time: None,
            interval: None,
            min_interval: None,
            error_count: 0,
        }
    }

    /// Determines whether we should announce to the tracker at the given time,
    /// based on when we last announced.
    ///
    /// Later this function should take into consideration the client's minimum
    /// announce frequency settings.
    fn should_announce(&self, t: Instant) -> bool {
        if let Some(last_announce_time) = self.last_announce_time {
            let min_next_announce_time =
                last_announce_time + self.interval.unwrap_or_default();
            t > min_next_announce_time
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
            let min_next_announce_time =
                last_announce_time + self.min_interval.unwrap_or_default();
            t > min_next_announce_time
        } else {
            true
        }
    }
}

/// The minimum number of peers we want to keep in torrent at all times.
/// This will be configurable later.
const MIN_REQUESTED_PEER_COUNT: usize = 10;
/// The max number of connected peers torrent should have.
// TODO: make this configurable
const MAX_CONNECTED_PEER_COUNT: usize = 50;

/// After this many attempts, we stop announcing to tracker.
const TRACKER_ERROR_THRESHOLD: usize = 10;
