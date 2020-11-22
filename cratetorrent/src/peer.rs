use std::{
    collections::HashSet,
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
    torrent::TorrentContext, Bitfield, Block, BlockInfo,
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
    /// The result of reading a block from disk.
    Block(Block),
    /// Eventually shut down the peer session.
    Shutdown,
}

/// A stopped or active connection with another BitTorrent peer.
///
/// This entity implements the BitTorrent wire protocol: it is responsible for
/// exchanging the BitTorrent messages that drive a download.
/// It only concerns itself with the network aspect of things: disk IO, for
/// example, is delegated to the [disk task](crate::disk::DiskHandle).
///
/// A peer session may be started in two modes:
/// - outbound: for connecting to another BitTorrent peer;
/// - inbound: for starting a session from an existing incoming TCP connection.
///
/// The only difference in the above two is how the handshake is handled at the
/// beginning of the connection. From then on the session mechanisms are
/// identical.
///
/// # Important
///
/// For now only the BitTorrent v1 specification is implemented, without any
/// extensions.
pub(crate) struct PeerSession {
    /// Shared information of the torrent.
    torrent: Arc<TorrentContext>,
    /// The entity used to save downloaded file blocks to disk.
    disk: DiskHandle,
    /// The command channel on which peer session is being sent messages.
    ///
    /// A copy of this is kept within peer session as disk block reads are
    /// communicated back to session directly via its command port. For this, we
    /// need to pass a copy of the sender with each block read to the disk task.
    cmd_chan: Sender,
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
    outgoing_requests: HashSet<BlockInfo>,
    /// The requests we got from peer.
    ///
    /// The request's entry is removed from here when the block is transmitted
    /// or when the peer cancels it.
    incoming_requests: HashSet<BlockInfo>,
}

impl PeerSession {
    /// Creates a new session with the peer at the given address.
    ///
    /// # Important
    ///
    /// This constructor only initializes the session components but does not
    /// actually start it. See [`Self::start`].
    pub fn new(
        torrent: Arc<TorrentContext>,
        disk: DiskHandle,
        addr: SocketAddr,
    ) -> (Self, Sender) {
        let (cmd_chan, cmd_port) = mpsc::unbounded_channel();
        (
            Self {
                torrent,
                disk,
                cmd_chan: cmd_chan.clone(),
                cmd_port: cmd_port.fuse(),
                addr,
                state: SessionState::default(),
                outgoing_requests: HashSet::new(),
                incoming_requests: HashSet::new(),
            },
            cmd_chan,
        )
    }

    /// Starts an outbound peer session.
    ///
    /// This method tries to connect to the peer at the address given in the
    /// constructor, send a handshake, and start the session.
    /// It returns if the connection is closed or an error occurs.
    pub async fn start_outbound(&mut self) -> Result<()> {
        peer_info!(self, "Starting outbound session");

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
                id: peer_handshake.peer_id,
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

            // enter the piece availability exchange state
            self.state.connection = ConnectionState::AvailabilityExchange;
            peer_info!(self, "Session state: {:?}", self.state.connection);

            // run the session
            self.run(socket).await?;
        }
        // TODO(https://github.com/mandreyel/cratetorrent/issues/20): handle
        // not recieving anything with an error rather than an Ok(())

        Ok(())
    }

    /// Starts an inbound peer session from an existing TCP connection.
    ///
    /// The method waits for the peer to send its handshake, responds
    /// with a handshake, and starts the session.
    /// It returns if the connection is closed or an error occurs.
    pub async fn start_inbound(&mut self, socket: TcpStream) -> Result<()> {
        peer_info!(self, "Starting inbound session");

        let mut socket = Framed::new(socket, HandshakeCodec);

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
                id: peer_handshake.peer_id,
                pieces: None,
            });

            // we reply with the handshake
            self.state.connection = ConnectionState::Handshaking;
            let handshake =
                Handshake::new(self.torrent.info_hash, self.torrent.client_id);
            peer_info!(self, "Sending handshake");
            self.state.uploaded_protocol_counter += handshake.len();
            socket.send(handshake).await?;

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

            // enter the piece availability exchange state
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

        // This is the beginning of the session, which is the only time
        // a peer is allowed to advertise their pieces. If we have pieces
        // available, send a bitfield message.
        {
            let piece_picker_guard = self.torrent.piece_picker.read().await;
            let own_pieces = piece_picker_guard.own_pieces();
            if own_pieces.any() {
                peer_info!(self, "Sending piece availability");
                sink.send(Message::Bitfield(own_pieces.clone())).await?;
            }
        }

        // used for collecting session stats every second
        let mut loop_timer = time::interval(Duration::from_secs(1)).fuse();

        // start the loop for receiving messages from peer and commands from
        // other parts of the engine
        loop {
            select! {
                _instant = loop_timer.select_next_some() => {
                    self.tick(&mut sink).await?;
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
                            // it's not mandatory to send a bitfield message
                            // right after the handshake
                            self.handle_msg(&mut sink, msg).await?;
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
                        Command::Block(block)=> {
                            self.send_block(&mut sink, block).await?;
                        }
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

        // register peer's pieces with piece picker and determine interest in it
        self.state.is_interested = self
            .torrent
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
                    // TODO(https://github.com/mandreyel/cratetorrent/issues/60):
                    // impleent the proper unchoke algorithm in `Torrent`
                    peer_info!(self, "Unchoking peer");
                    sink.send(Message::Unchoke).await?;
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
                self.handle_block_msg(block_info, data.into_owned()).await?;

                // we may be able to make more requests now that a block has
                // arrived
                self.make_requests(sink).await?;
            }
            // TODO: implement these
            Message::Request(block_info) => {
                self.handle_request_msg(block_info).await?;
            }
            Message::Have { piece_index } => {
                peer_info!(
                    self,
                    "Received 'have' message for piece {}",
                    piece_index
                );
                // need to recalculate interest with each received piece
                self.state.is_interested = self
                    .torrent
                    .piece_picker
                    .write()
                    .await
                    .register_piece_availability(piece_index)?;
            }
            Message::Cancel(block_info) => {
                peer_info!(
                    self,
                    "Received 'cancel' message for {}",
                    block_info
                );
                self.incoming_requests.remove(&block_info);
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
        let mut requests = Vec::new();
        let target_request_queue_len =
            self.state.target_request_queue_len.unwrap_or_default();

        // If we have active downloads, prefer to continue those. This will
        // result in less in-progress pieces.
        for download in self.torrent.downloads.write().await.values_mut() {
            // check and calculate the number of requests we can make now
            let outgoing_request_count =
                requests.len() + self.outgoing_requests.len();
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
            download_write_guard.pick_blocks(to_request_count, &mut requests);
        }

        // while we can make more requests we start new download(s)
        loop {
            let outgoing_request_count =
                requests.len() + self.outgoing_requests.len();
            // our outgoing request queue shouldn't exceed the allowed request
            // queue size
            if outgoing_request_count >= target_request_queue_len {
                break;
            }
            let to_request_count =
                target_request_queue_len - outgoing_request_count;

            peer_debug!(self, "Starting new piece download");

            if let Some(index) =
                self.torrent.piece_picker.write().await.pick_piece()
            {
                peer_info!(self, "Picked piece {}", index);

                let mut download = PieceDownload::new(
                    index,
                    self.torrent.storage.piece_len(index)?,
                );

                download.pick_blocks(to_request_count, &mut requests);
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

        if !requests.is_empty() {
            peer_info!(
                self,
                "Requesting {} block(s) ({} pending)",
                requests.len(),
                self.outgoing_requests.len()
            );
            self.state.last_outgoing_request_time = Some(Instant::now());
            // make the actual requests
            for req in requests.into_iter() {
                self.outgoing_requests.insert(req);
                // TODO: batch these in a single syscall, or is this already
                // being done by the tokio codec type?
                sink.send(Message::Request(req)).await?;
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

        // remove block from our pending requests queue
        let was_present = self.outgoing_requests.remove(&block_info);

        // find block in the list of pending requests
        if !was_present {
            peer_warn!(self, "Received not requested block: {:?}", block_info);
            // silently ignore this block if we didn't expected it
            //
            // TODO(https://github.com/mandreyel/cratetorrent/issues/10): In
            // the future we could add logic that only accepts blocks within
            // a window after the last request. If not done, peer could DoS
            // us by sending unwanted blocks repeatedly.
            return Ok(());
        }

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
            self.torrent
                .piece_picker
                .write()
                .await
                .received_piece(piece_index);
        }
    }

    /// Handles the peer request message.
    /// TODO: docs
    async fn handle_request_msg(
        &mut self,
        block_info: BlockInfo,
    ) -> Result<()> {
        peer_info!(self, "Received request: {:?}", block_info);

        // check if peer is not already requesting this block
        if self.incoming_requests.contains(&block_info) {
            // TODO: if peer keeps spamming us, close connection
            peer_warn!(self, "Peer sent duplicate block request");
            return Ok(());
        }

        // TODO: validate block info

        peer_info!(self, "Issuing disk IO read for block: {:?}", block_info);
        self.incoming_requests.insert(block_info);

        // validate and save the block to disk by sending a write command to the
        // disk task
        self.disk.read_block(
            self.torrent.id,
            block_info,
            self.cmd_chan.clone(),
        )?;

        Ok(())
    }

    async fn send_block(
        &mut self,
        sink: &mut SplitSink<Framed<TcpStream, PeerCodec>, Message>,
        block: Block,
    ) -> Result<()> {
        let info = block.info();
        peer_info!(self, "Read from disk {}", info);

        // remove peer's pending request
        let was_present = self.incoming_requests.remove(&info);

        // check if the request hasn't been canceled yet
        if !was_present {
            peer_warn!(self, "No matching request entry for {}", info);
            return Ok(());
        }

        // if it hasn't, send the data to peer
        peer_info!(self, "Sending {}", info);
        sink.send(Message::Block {
            piece_index: block.piece_index,
            offset: block.offset,
            data: block.data,
        })
        .await?;
        peer_info!(self, "Sent {}", info);

        // update download stats
        self.state.update_upload_stats(info.len);

        Ok(())
    }
}
