use {
    futures::{
        select,
        stream::{Fuse, SplitSink},
        SinkExt, StreamExt,
    },
    std::time::{Duration, Instant},
    std::{net::SocketAddr, sync::Arc},
    tokio::{
        net::TcpStream,
        sync::{
            mpsc::{self, UnboundedReceiver, UnboundedSender},
            RwLock,
        },
        time,
    },
    tokio_util::codec::{Framed, FramedParts},
};

use {
    crate::{
        disk::DiskHandle, download::PieceDownload, error::*,
        piece_picker::PiecePicker, torrent::SharedStatus, Bitfield, BlockInfo,
    },
    codec::*,
    state::*,
};

mod codec;
mod state;

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
    /// These are the active piece downloads in which this session is
    /// participating.
    downloads: Vec<PieceDownload>,
    /// Our pending requests that we sent to peer. It represents the blocks that
    /// we are expecting. Thus, if we receive a block that is not in this list,
    /// it is dropped. If we receive a block whose request entry is in here, the
    /// entry is removed.
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
                downloads: Vec::new(),
                outgoing_requests: Vec::new(),
            },
            cmd_chan,
        )
    }

    /// Starts the peer session and returns if the connection is closed or an
    /// error occurs.
    pub async fn start(&mut self) -> Result<()> {
        log::info!("Starting peer {} session", self.addr);

        log::info!("Connecting to peer {}", self.addr);
        self.state.connection = ConnectionState::Connecting;
        let socket = TcpStream::connect(self.addr).await?;
        log::info!("Connected to peer {}", self.addr);

        let mut socket = Framed::new(socket, HandshakeCodec);

        // this is an outbound connection, so we have to send the first
        // handshake
        self.state.connection = ConnectionState::Handshaking;
        let handshake =
            Handshake::new(self.torrent.info_hash, self.torrent.client_id);
        log::info!("Sending handshake to peer {}", self.addr);
        self.state.uploaded_protocol_counter += handshake.len();
        socket.send(handshake).await?;

        // receive peer's handshake
        log::info!("Waiting for peer {} handshake", self.addr);
        if let Some(peer_handshake) = socket.next().await {
            let peer_handshake = peer_handshake?;
            log::info!("Received handshake from peer {}", self.addr);
            log::trace!("Peer {} handshake: {:?}", self.addr, peer_handshake);
            // codec should only return handshake if the protocol string in it
            // is valid
            debug_assert_eq!(peer_handshake.prot, PROTOCOL_STRING.as_bytes());

            self.state.downloaded_protocol_counter += peer_handshake.len();

            // verify that the advertised torrent info hash is the same as ours
            if peer_handshake.info_hash != self.torrent.info_hash {
                log::info!("Peer {} handshake invalid info hash", self.addr);
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
            log::info!(
                "Peer {} session state: {:?}",
                self.addr,
                self.state.connection
            );

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
                    self.tick(instant.into_std());
                }
                msg = stream.select_next_some() => {
                    let msg = msg?;
                    log::debug!(
                        "Received message {} from peer {:?}",
                        self.addr,
                        msg.id()
                    );

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
                        log::info!(
                            "Peer {} session state: {:?}",
                            self.addr,
                            self.state.connection
                        );
                    } else {
                        self.handle_msg(&mut sink, msg).await?;
                    }
                }
                cmd = self.cmd_port.select_next_some() => {
                    match cmd {
                        Command::Shutdown => {
                            log::info!("Shutting down peer {} session", self.addr);
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn tick(&mut self, _instant: Instant) {
        // TODO(https://github.com/mandreyel/cratetorrent/issues/43): check peer timeout

        // TODO(https://github.com/mandreyel/cratetorrent/issues/42): send keep-alive

        // update status
        self.state.tick();

        log::info!(
            "[Peer {}] download rate: {} b/s (peak: {} b/s, total: {} b) \
            queue: {}, rtt: {} ms (~{} s)",
            self.addr,
            self.state.downloaded_payload_counter.avg(),
            self.state.downloaded_payload_counter.peak(),
            self.state.downloaded_payload_counter.total(),
            self.state.target_request_queue_len.unwrap_or(0),
            self.state.avg_request_rtt.mean().as_millis(),
            self.state.avg_request_rtt.mean().as_secs(),
        );
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
        log::info!("Handling peer {} Bitfield message", self.addr);
        log::trace!("Bitfield: {:?}", bitfield);

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

        // register peer's pieces with piece picker
        let mut piece_picker = self.piece_picker.write().await;
        self.state.is_interested =
            piece_picker.register_availability(&bitfield)?;
        debug_assert!(self.state.is_interested);
        if let Some(peer_info) = &mut self.state.peer {
            peer_info.pieces = Some(bitfield);
        }

        // send interested message to peer
        log::info!("Interested in peer {}", self.addr);
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
                log::info!(
                    "Peer {} sent bitfield message not after handshake",
                    self.addr
                );
                return Err(Error::BitfieldNotAfterHandshake);
            }
            Message::KeepAlive => {
                log::info!("Peer {} sent keep alive", self.addr);
            }
            Message::Choke => {
                if !self.state.is_choked {
                    log::info!("Peer {} choked us", self.addr);
                    // since we're choked we don't expect to receive blocks
                    // for our pending requests
                    self.outgoing_requests.clear();
                    self.state.is_choked = true;
                }
            }
            Message::Unchoke => {
                if self.state.is_choked {
                    log::info!("Peer {} unchoked us", self.addr);
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
                    log::info!("Peer {} is interested", self.addr);
                    self.state.is_peer_interested = true;
                }
            }
            Message::NotInterested => {
                if self.state.is_peer_interested {
                    log::info!("Peer {} is not interested", self.addr);
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
        log::trace!("Making requests to peer {}", self.addr);

        // FIXME: sometimes we don't seem to be making any requests at all

        // TODO: optimize this by preallocating the vector in self
        let mut blocks = Vec::new();

        // If we have active downloads, prefer to continue those. This will
        // result in less in-progress pieces.
        for download in self.downloads.iter_mut() {
            log::debug!(
                "Peer {} trying to continue download {}",
                self.addr,
                download.piece_index()
            );

            let outgoing_request_count =
                blocks.len() + self.outgoing_requests.len();
            // our outgoing request queue shouldn't exceed the allowed request
            // queue size
            debug_assert!(
                self.state.target_request_queue_len.unwrap_or_default()
                    >= outgoing_request_count
            );
            // the number of requests we can make now
            let to_request_count =
                self.state.target_request_queue_len.unwrap_or_default()
                    - outgoing_request_count;
            if to_request_count == 0 {
                break;
            }

            // TODO: should we not check first that we aren't already
            // downloading all of the piece's blocks?

            download.pick_blocks(to_request_count, &mut blocks);
        }

        // while we can make more requests we start new download(s)
        loop {
            let outgoing_request_count =
                blocks.len() + self.outgoing_requests.len();
            // our outgoing request queue shouldn't exceed the allowed request
            // queue size
            debug_assert!(
                self.state.target_request_queue_len.unwrap_or_default()
                    >= outgoing_request_count
            );
            let to_request_count =
                self.state.target_request_queue_len.unwrap_or_default()
                    - outgoing_request_count;
            if to_request_count == 0 {
                break;
            }

            log::debug!("Session {} starting new piece download", self.addr);

            let mut piece_picker = self.piece_picker.write().await;
            if let Some(index) = piece_picker.pick_piece() {
                log::info!("Session {} picked piece {}", self.addr, index);

                let mut download = PieceDownload::new(
                    index,
                    self.torrent.storage.piece_len(index)?,
                );

                download.pick_blocks(to_request_count, &mut blocks);
                // save download
                self.downloads.push(download);
            } else {
                log::debug!(
                    "Could not pick more pieces from peer {}",
                    self.addr
                );
                break;
            }
        }

        if !blocks.is_empty() {
            log::info!(
                "Requesting {} block(s) from peer {} ({} pending)",
                blocks.len(),
                self.addr,
                self.outgoing_requests.len()
            );
            // save current volley of requests
            self.outgoing_requests.extend_from_slice(&blocks);
            // make the actual requests
            for block in blocks.iter() {
                // TODO: batch these in a single syscall
                sink.send(Message::Request(*block)).await?;
            }

            self.state.last_outgoing_request_time = Some(Instant::now());
            self.state.uploaded_protocol_counter +=
                blocks.len() as u64 * MessageId::Request.header_len();
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
        log::info!("Received block from peer {}: {:?}", self.addr, block_info);

        // find block in the list of pending requests
        let request_pos = match self
            .outgoing_requests
            .iter()
            .position(|b| *b == block_info)
        {
            Some(pos) => pos,
            None => {
                log::warn!(
                    "Peer {} sent not requested block: {:?}",
                    self.addr,
                    block_info,
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
        let download_pos = self
            .downloads
            .iter()
            .position(|d| d.piece_index() == block_info.piece_index);
        // this fires as a result of a broken invariant: we
        // shouldn't have an entry in `outgoing_requests` without a
        // corresponding entry in `downloads`
        //
        // TODO(https://github.com/mandreyel/cratetorrent/issues/11): can we
        // handle this without unwrapping?
        debug_assert!(download_pos.is_some());
        let download_pos = download_pos.unwrap();
        let download = &mut self.downloads[download_pos];
        download.received_block(block_info);

        // finish download of piece if this was the last missing block in it
        if download.is_complete() {
            log::info!(
                "Finished piece {} via peer {}",
                block_info.piece_index,
                self.addr
            );
            // register received piece
            self.piece_picker
                .write()
                .await
                .received_piece(block_info.piece_index);
            // remove piece download from `downloads`
            self.downloads.remove(download_pos);
        }

        // update download stats
        self.state.update_download_stats(block_info.len);

        // validate and save the block to disk by sending a write command to the
        // disk task
        self.disk.write_block(self.torrent.id, block_info, data)?;

        Ok(())
    }
}

/// The channel on which torrent can send a command to the peer session task.
pub(crate) type Sender = UnboundedSender<Command>;
type Receiver = UnboundedReceiver<Command>;

/// The commands peer session can receive.
pub(crate) enum Command {
    /// Eventually shut down the peer session.
    Shutdown,
}
