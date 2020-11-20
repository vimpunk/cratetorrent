use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{
    select,
    stream::{Fuse, SplitSink},
    SinkExt, StreamExt,
};
use tokio::{
    net::TcpStream,
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
    time,
};
use tokio_util::codec::{Framed, FramedParts};

use crate::{
    disk::DiskHandle, download::PieceDownload, error::*,
    piece_picker::PiecePicker, torrent::SharedStatus, Bitfield, BlockInfo,
};
use codec::*;
use state::*;

mod codec;
mod state;
#[macro_use]
mod peer_log;

/// The channel on which torrent can send a command to the peer session task.
pub(crate) type Sender = UnboundedSender<Command>;
type Receiver = UnboundedReceiver<Command>;

/// The commands peer session can receive.
pub(crate) enum Command {
    /// Eventually shut down the peer session.
    Shutdown,
}

pub(crate) struct PeerSession {
    /// Shared information of the torrent.
    torrent: Arc<SharedStatus>,
    /// The piece picker picks the next most optimal piece to download and is
    /// shared by other entities in the same torrent.
    piece_picker: Arc<RwLock<PiecePicker>>,
    /// The entity used to save downloaded file blocks to disk.
    disk: DiskHandle,
    /// The port on which peer session receives commands.
    cmd_port: Fuse<Receiver>,
    /// The remote address of the peer.
    addr: SocketAddr,
    /// Session related information.
    state: SessionState,
    /// Our pending requests that we sent to peer. It represents the blocks that
    /// we are expecting. Thus, if we receive a block that is not in this list,
    /// we need to drop it. If we receive a block whose request entry is in
    /// here, the entry is removed.
    ///
    /// Since the Fast extension is not supported (yet), this is emptied when
    /// we're choked, as in that case we don't expect outstanding requests to be
    /// served.
    ///
    /// Note that if a reuest for a piece's block is in this queue, there _must_
    /// be a corresponding entry for the piece download in `downloads`.
    // TODO(https://github.com/mandreyel/cratetorrent/issues/11): Can we store
    // this information in just PieceDownload so that we don't have to enforce
    // this invariant (keeping in mind that later PieceDownloads will be shared
    // among PeerSessions)?
    outgoing_requests: Vec<BlockInfo>,
}

impl PeerSession {
    /// Creates a new outbound session with the peer at the given address.
    ///
    /// The peer needs to be a seed in order for us to download a file through
    /// this peer session, otherwise the session is aborted with an error.
    pub fn outbound(
        torrent: Arc<SharedStatus>,
        piece_picker: Arc<RwLock<PiecePicker>>,
        disk: DiskHandle,
        addr: SocketAddr,
    ) -> (Self, Sender) {
        let (cmd_chan, cmd_port) = mpsc::unbounded_channel();
        (
            Self {
                torrent,
                piece_picker,
                disk,
                cmd_port: cmd_port.fuse(),
                addr,
                state: SessionState::default(),
                outgoing_requests: Vec::new(),
            },
            cmd_chan,
        )
    }

    /// Starts the peer session and returns if the connection is closed or an
    /// error occurs.
    pub async fn start(&mut self) -> Result<()> {
        peer_info!(self, "Starting session");

        peer_info!(self, "Connecting to peer");
        self.state.connection = ConnectionState::Connecting;
        let socket = TcpStream::connect(self.addr).await?;
        peer_info!(self, "Connected to peer");

        let mut socket = Framed::new(socket, HandshakeCodec);

        // this is an outbound connection, so we have to send the first
        // handshake
        self.state.connection = ConnectionState::Handshaking;
        let handshake =
            Handshake::new(self.torrent.info_hash, self.torrent.client_id);
        peer_info!(self, "Sending handshake");
        self.state.uploaded_protocol_counter += handshake.len();
        socket.send(handshake).await?;

        // receive peer's handshake
        peer_info!(self, "Waiting for peer handshake");
        if let Some(peer_handshake) = socket.next().await {
            let peer_handshake = peer_handshake?;
            peer_info!(self, "Received peer handshake");
            peer_trace!(self, "Peer handshake: {:?}", peer_handshake);
            // codec should only return handshake if the protocol string in it
            // is valid
            debug_assert_eq!(peer_handshake.prot, PROTOCOL_STRING.as_bytes());

            self.state.downloaded_protocol_counter += peer_handshake.len();

            // verify that the advertised torrent info hash is the same as ours
            if peer_handshake.info_hash != self.torrent.info_hash {
                peer_info!(self, "Peer handshake invalid info hash");
                // abort session, info hash is invalid
                return Err(Error::InvalidPeerInfoHash);
            }

            // set basic peer information
            self.state.peer = Some(PeerInfo {
                id: handshake.peer_id,
                pieces: None,
            });

            // now that we have the handshake, we need to switch to the peer
            // message codec and save the socket in self (note that we need to
            // keep the buffer from the original codec as it may contain bytes
            // of any potential message the peer may have sent after the
            // handshake)
            let old_parts = socket.into_parts();
            let mut new_parts = FramedParts::new(old_parts.io, PeerCodec);
            // reuse buffers of previous codec
            new_parts.read_buf = old_parts.read_buf;
            new_parts.write_buf = old_parts.write_buf;
            let socket = Framed::from_parts(new_parts);

            // enter the piece availability exchange state until peer sends a
            // bitfield (we don't send one as we currently only implement
            // downloading so we cannot have piece availability until multiple
            // peer connections or resuming a torrent is implemented)
            self.state.connection = ConnectionState::AvailabilityExchange;
            peer_info!(self, "Session state: {:?}", self.state.connection);

            // run the session
            self.run(socket).await?;
        }
        // TODO(https://github.com/mandreyel/cratetorrent/issues/20): handle
        // not recieving anything with an error rather than an Ok(())

        Ok(())
    }

    /// Runs the session after connection to peer is established.
    ///
    /// This is the main session "loop" and performs the core of the session
    /// logic: exchange of messages, timeout logic, etc.
    async fn run(
        &mut self,
        socket: Framed<TcpStream, PeerCodec>,
    ) -> Result<()> {
        // split the sink and stream so that we can pass the sink while holding
        // a reference to the stream in the loop
        let (mut sink, stream) = socket.split();
        let mut stream = stream.fuse();

        // used for collecting session stats every second
        let mut loop_timer = time::interval(Duration::from_secs(1)).fuse();

        // start the loop for receiving messages from peer and commands from
        // other parts of the engine
        loop {
            select! {
                instant = loop_timer.select_next_some() => {
                    self.tick(&mut sink, instant.into_std()).await?;
                }
                msg = stream.select_next_some() => {
                    let msg = msg?;
                    peer_debug!(self, "Received message {:?}", msg.id());

                    // handle bitfield message separately as it may only be
                    // received directly after the handshake (later once we
                    // implement the FAST extension, there will be other piece
                    // availability related messages to handle)
                    if self.state.connection == ConnectionState::AvailabilityExchange {
                        if let Message::Bitfield(bitfield) = msg {
                            self.handle_bitfield_msg(&mut sink, bitfield).await?;
                        } else {
                            // since we expect peer to be a seed, we *must* get
                            // a bitfield message, as otherwise we assume the
                            // peer to be a leech with no pieces to share (which
                            // is not good for our purposes of downloading
                            // a file)
                            log::warn!(
                                "Peer {} hasn't sent bitfield, cannot download",
                                self.addr
                            );
                            return Err(Error::PeerNotSeed);
                        }

                        // enter connected state
                        self.state.connection = ConnectionState::Connected;
                        peer_info!(self,
                            "Session state: {:?}",
                            self.state.connection
                        );
                    } else {
                        self.handle_msg(&mut sink, msg).await?;
                    }
                }
                cmd = self.cmd_port.select_next_some() => {
                    match cmd {
                        Command::Shutdown => {
                            peer_info!(self, "Shutting down session");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn tick(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, Message>,
        _instant: Instant,
    ) -> Result<()> {
        // TODO(https://github.com/mandreyel/cratetorrent/issues/43): check peer
        // inactivity timeout

        // resent requests if we have pending requests and more time has elapsed
        // since the last request than the current timeout value
        if let Some(last_outgoing_request_time) =
            self.state.last_outgoing_request_time
        {
            let elapsed_since_last_request = Instant::now()
                .saturating_duration_since(last_outgoing_request_time);
            let request_timeout = self.state.request_timeout();
            peer_debug!(
                self,
                "Checking request timeout \
                (last {} ms ago, timeout: {} ms)",
                elapsed_since_last_request.as_millis(),
                request_timeout.as_millis()
            );
            if elapsed_since_last_request > request_timeout {
                peer_warn!(
                    self,
                    "Timeout after {} ms (timeouts: {})",
                    elapsed_since_last_request.as_millis(),
                    self.state.timed_out_request_count + 1,
                );
                self.state.register_request_timeout();
                // Cancel all requests and re-issue a single one (since we can
                // only request a single block now). Start by freeing up the
                // block in its piece download.
                for block in self.outgoing_requests.iter() {
                    // this fires as a result of a broken invariant: we
                    // shouldn't have an entry in `outgoing_requests` without a
                    // corresponding entry in `downloads`
                    //
                    // TODO(https://github.com/mandreyel/cratetorrent/issues/11):
                    // can we handle this without unwrapping?
                    self.torrent
                        .downloads
                        .write()
                        .await
                        .get_mut(&block.piece_index)
                        .expect("No corresponding PieceDownload for request")
                        .write()
                        .await
                        .cancel_request(block);
                }
                self.outgoing_requests.clear();
                // try to make a request
                self.make_requests(sink).await?;
            }
        }

        // TODO(https://github.com/mandreyel/cratetorrent/issues/42): send
        // keep-alive

        // update status
        self.state.tick();

        peer_info!(
            self,
            "Info: download rate: {} b/s (peak: {} b/s, total: {} b) \
            queue: {}, rtt: {} ms (~{} s)",
            self.state.downloaded_payload_counter.avg(),
            self.state.downloaded_payload_counter.peak(),
            self.state.downloaded_payload_counter.total(),
            self.state.target_request_queue_len.unwrap_or(0),
            self.state.avg_request_rtt.mean().as_millis(),
            self.state.avg_request_rtt.mean().as_secs(),
        );

        Ok(())
    }

    /// Handles a message expected in the `AvailabilityExchange` state
    /// (currently only the bitfield message).
    async fn handle_bitfield_msg(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, Message>,
        mut bitfield: Bitfield,
    ) -> Result<()> {
        debug_assert_eq!(
            self.state.connection,
            ConnectionState::AvailabilityExchange
        );
        peer_info!(self, "Handling peer Bitfield message");
        peer_trace!(self, "Bitfield: {:?}", bitfield);

        // The bitfield raw data that is sent over the wire may be longer than
        // the logical pieces it represents, if there the number of pieces in
        // torrent is not a multiple of 8. Therefore, we need to slice off the
        // last part of the bitfield.
        //
        // According to the spec if the remainder contains any non-zero
        // bits, we need to abort the connection. Not sure if this is too
        // strict, there doesn't seem much harm in it so we skip the check.
        bitfield.resize(self.torrent.storage.piece_count, false);

        // if peer is not a seed, we abort the connection as we only
        // support downloading and for that we must be connected to
        // a seed (otherwise we couldn't download the whole torrent)
        if !bitfield.all() {
            log::warn!("Peer {} is not a seed, cannot download", self.addr);
            return Err(Error::PeerNotSeed);
        }

        // register peer's pieces with piece picker and determine interest in it
        self.state.is_interested = self
            .piece_picker
            .write()
            .await
            .register_availability(&bitfield)?;
        debug_assert!(self.state.is_interested);
        if let Some(peer_info) = &mut self.state.peer {
            peer_info.pieces = Some(bitfield);
        }

        // send interested message to peer
        peer_info!(self, "Interested in peer");
        sink.send(Message::Interested).await?;
        self.state.uploaded_protocol_counter +=
            MessageId::Interested.header_len();

        Ok(())
    }

    /// Handles messages expected in the `Connected` state.
    async fn handle_msg(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, Message>,
        msg: Message,
    ) -> Result<()> {
        // record protocol message size
        self.state.downloaded_protocol_counter += msg.protocol_len();
        match msg {
            Message::Bitfield(_) => {
                peer_info!(
                    self,
                    "Peer sent bitfield message not after handshake"
                );
                return Err(Error::BitfieldNotAfterHandshake);
            }
            Message::KeepAlive => {
                peer_info!(self, "Peer sent keep alive");
            }
            Message::Choke => {
                if !self.state.is_choked {
                    peer_info!(self, "Peer choked us");
                    // since we're choked we don't expect to receive blocks
                    // for our pending requests
                    self.outgoing_requests.clear();
                    self.state.is_choked = true;
                }
            }
            Message::Unchoke => {
                if self.state.is_choked {
                    peer_info!(self, "Peer unchoked us");
                    self.state.is_choked = false;

                    // if we're interested, start sending requests
                    if self.state.is_interested {
                        self.state.prepare_for_download();
                        // now that we are allowed to request blocks, start the
                        // download pipeline if we're interested
                        self.make_requests(sink).await?;
                    }
                }
            }
            Message::Interested => {
                if !self.state.is_peer_interested {
                    peer_info!(self, "Peer became interested");
                    self.state.is_peer_interested = true;
                }
            }
            Message::NotInterested => {
                if self.state.is_peer_interested {
                    peer_info!(self, "Peer no longer interested");
                    self.state.is_peer_interested = false;
                }
            }
            Message::Block {
                piece_index,
                offset,
                data,
            } => {
                let block_info = BlockInfo {
                    piece_index,
                    offset,
                    len: data.len() as u32,
                };
                self.handle_block_msg(block_info, data).await?;

                // we may be able to make more requests now that a block has
                // arrived
                self.make_requests(sink).await?;
            }
            // these messages are not expected until seed functionality is added
            Message::Have { .. } => {
                log::warn!(
                    "Seed {} sent unexpected message: {:?}",
                    self.addr,
                    MessageId::Have
                );
            }
            Message::Request(_) => {
                log::warn!(
                    "Seed {} sent unexpected message: {:?}",
                    self.addr,
                    MessageId::Request
                );
            }
            Message::Cancel(_) => {
                log::warn!(
                    "Seed {} sent unexpected message: {:?}",
                    self.addr,
                    MessageId::Cancel
                );
            }
        }

        Ok(())
    }

    /// Fills the session's download pipeline with the optimal number of
    /// requests.
    ///
    /// To see what this means, please refer to the
    /// `Status::best_request_queue_len` or the relevant section in DESIGN.md.
    async fn make_requests(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, Message>,
    ) -> Result<()> {
        peer_trace!(self, "Making requests");

        // FIXME: sometimes we don't seem to be making any requests at all

        // TODO: optimize this by preallocating the vector in self
        let mut blocks = Vec::new();
        let target_request_queue_len =
            self.state.target_request_queue_len.unwrap_or_default();

        // If we have active downloads, prefer to continue those. This will
        // result in less in-progress pieces.
        for download in self.torrent.downloads.write().await.values_mut() {
            // check and calculate the number of requests we can make now
            let outgoing_request_count =
                blocks.len() + self.outgoing_requests.len();
            // our outgoing request queue shouldn't exceed the allowed request
            // queue size
            if outgoing_request_count >= target_request_queue_len {
                break;
            }
            let to_request_count =
                target_request_queue_len - outgoing_request_count;

            // TODO: should we check first that we aren't already downloading
            // all of the piece's blocks? requires read then write

            let mut download_write_guard = download.write().await;
            peer_trace!(
                self,
                "Trying to continue download {}",
                download_write_guard.piece_index()
            );
            download_write_guard.pick_blocks(to_request_count, &mut blocks);
        }

        // while we can make more requests we start new download(s)
        loop {
            let outgoing_request_count =
                blocks.len() + self.outgoing_requests.len();
            // our outgoing request queue shouldn't exceed the allowed request
            // queue size
            if outgoing_request_count >= target_request_queue_len {
                break;
            }
            let to_request_count =
                target_request_queue_len - outgoing_request_count;

            peer_debug!(self, "Starting new piece download");

            if let Some(index) = self.piece_picker.write().await.pick_piece() {
                peer_info!(self, "Picked piece {}", index);

                let mut download = PieceDownload::new(
                    index,
                    self.torrent.storage.piece_len(index)?,
                );

                download.pick_blocks(to_request_count, &mut blocks);
                // save download
                self.torrent
                    .downloads
                    .write()
                    .await
                    .insert(index, RwLock::new(download));
            } else {
                peer_debug!(
                    self,
                    "Could not pick more pieces (pending: \
                    pieces: {}, blocks: {})",
                    self.torrent.downloads.read().await.len(),
                    self.outgoing_requests.len(),
                );
                break;
            }
        }

        if !blocks.is_empty() {
            peer_info!(
                self,
                "Requesting {} block(s) ({} pending)",
                blocks.len(),
                self.outgoing_requests.len()
            );
            self.state.last_outgoing_request_time = Some(Instant::now());
            // save current volley of requests
            self.outgoing_requests.extend_from_slice(&blocks);
            // make the actual requests
            for block in blocks.iter() {
                // TODO: batch these in a single syscall, or is this already
                // being done by the tokio codec type?
                sink.send(Message::Request(*block)).await?;
                self.state.uploaded_protocol_counter +=
                    MessageId::Request.header_len();
            }
        }

        Ok(())
    }

    /// Verifies block validity, registers the download (and finishes a piece
    /// download if this was the last missing block in piece) and updates
    /// statistics about the download.
    async fn handle_block_msg(
        &mut self,
        block_info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        peer_info!(self, "Received block: {:?}", block_info);

        // find block in the list of pending requests
        let request_pos = match self
            .outgoing_requests
            .iter()
            .position(|b| *b == block_info)
        {
            Some(pos) => pos,
            None => {
                peer_warn!(
                    self,
                    "Received not requested block: {:?}",
                    block_info
                );
                // silently ignore this block if we didn't expected
                // it
                //
                // TODO(https://github.com/mandreyel/cratetorrent/issues/10): In
                // the future we could add logic that only accepts blocks within
                // a window after the last request. If not done, peer could DoS
                // us by sending unwanted blocks repeatedly.
                return Ok(());
            }
        };

        // remove block from our pending requests queue
        self.outgoing_requests.remove(request_pos);

        // mark the block as downloaded with its respective piece
        // download instance
        self.register_downloaded_block(&block_info).await;

        // update download stats
        self.state.update_download_stats(block_info.len);

        // validate and save the block to disk by sending a write command to the
        // disk task
        self.disk.write_block(self.torrent.id, block_info, data)?;

        Ok(())
    }

    /// Marks the newly downloaded block in the piece picker and its piece
    /// download instance.
    ///
    /// If the block completes the piece, the piece download is removed from the
    /// shared download map and the piece is marked as complete in the piece
    /// picker.
    async fn register_downloaded_block(&self, block_info: &BlockInfo) {
        let mut downloads_write_guard = self.torrent.downloads.write().await;

        let piece_index = block_info.piece_index;
        let download = downloads_write_guard.get_mut(&piece_index);
        // this fires as a result of a broken invariant: we
        // shouldn't have an entry in `outgoing_requests` without a
        // corresponding entry in `downloads`
        //
        // TODO(https://github.com/mandreyel/cratetorrent/issues/11): can we
        // handle this without unwrapping?
        let download =
            download.expect("No corresponding PieceDownload for request");
        download.write().await.received_block(&block_info);

        // finish download of piece if this was the last missing block in it
        if download.read().await.is_complete() {
            peer_info!(self, "Finished piece {}", piece_index);
            // remove piece download from `downloads`
            downloads_write_guard.remove(&piece_index);
            // drop the write guard to avoid holding two write locks that could
            // later cause deadlocks
            drop(downloads_write_guard);
            // register received piece
            self.piece_picker.write().await.received_piece(piece_index);
        }
    }
}
