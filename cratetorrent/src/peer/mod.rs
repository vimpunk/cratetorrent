mod codec;

use crate::error::*;
use crate::piece_picker::PiecePicker;
use crate::torrent::SharedTorrentInfo;
use codec::*;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio_util::codec::{Framed, FramedParts};

/// At any given time, a connection with a peer is in one of the below states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// The peer connection has not yet been connected or it had been connected
    /// before but has been stopped.
    Disconnected,
    /// The state during which the TCP connection is established.
    Connecting,
    /// The state after establishing the TCP connection and exchanging the
    /// initial BitTorrent handshake.
    Handshaking,
    /// This state is optional, it is used to verify that the bitfield exchange
    /// occurrs after the handshake and not later. It is set once the handshakes
    /// are exchanged and changed as soon as we receive the bitfield or the the
    /// first message that is not a bitfield. Any subsequent bitfield messages
    /// are rejected and the connection is dropped, as per the standard.
    AvailabilityExchange,
    /// This is the normal state of a peer session, in which any messages, apart
    /// from the 'handshake' and 'bitfield', may be exchanged.
    Connected,
}

/// The default (and initial) state of a peer session is `Disconnected`.
impl Default for State {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// The choke and interest status of a peer session.
///
/// By default, both sides of the connection start off as choked and not
/// interested in the other.
#[derive(Clone, Copy, Debug)]
struct Status {
    is_choked: bool,
    is_interested: bool,
    is_peer_choked: bool,
    is_peer_interested: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            is_choked: true,
            is_interested: false,
            is_peer_choked: true,
            is_peer_interested: false,
        }
    }
}

pub(crate) struct PeerSession {
    // The current state of the session.
    state: State,
    // Shared information of the torrent.
    torrent_info: Arc<SharedTorrentInfo>,
    piece_picker: Arc<RwLock<PiecePicker>>,
    // The remote address of the peer.
    addr: SocketAddr,
    // Session related information.
    status: Status,
}

impl PeerSession {
    // Creates a new outbound session with the peer at the given address.
    //
    // The peer needs to be a seed in order for us to download a file through
    // this peer session, otherwise the session is aborted with an error.
    pub fn outbound(
        torrent_info: Arc<SharedTorrentInfo>,
        piece_picker: Arc<RwLock<PiecePicker>>,
        addr: SocketAddr,
    ) -> Self {
        Self {
            state: State::default(),
            torrent_info,
            piece_picker,
            addr,
            status: Status::default(),
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        log::info!("Starting peer {} session", self.addr);

        log::info!("Connecting to peer {}", self.addr);
        self.state = State::Connecting;
        let socket = TcpStream::connect(self.addr).await?;
        log::info!("Connected to peer {}", self.addr);

        let mut socket = Framed::new(socket, HandshakeCodec);

        // this is an outbound connection, so we have to send the first
        // handshake
        self.state = State::Handshaking;
        let handshake = Handshake::new(
            self.torrent_info.info_hash,
            self.torrent_info.client_id,
        );
        log::info!("Sending handshake to peer {}", self.addr);
        socket.send(handshake).await?;

        // receive peer's handshake
        log::info!("Receiving handshake from peer {}", self.addr);
        if let Some(peer_handshake) = socket.next().await {
            let peer_handshake = peer_handshake?;
            log::info!("Received handshake from peer {}", self.addr);
            log::debug!("Peer {} handshake: {:?}", self.addr, peer_handshake);
            // codec should only return handshake if the protocol string in it
            // is valid
            debug_assert_eq!(peer_handshake.prot, PROTOCOL_STRING.as_bytes());

            // verify that the advertised torrent info hash is the same as ours
            if peer_handshake.info_hash != self.torrent_info.info_hash {
                log::info!("Peer {} handshake invalid info hash", self.addr);
                // abort session, info hash is invalid
                return Err(Error::InvalidPeerInfoHash);
            }

            // now that we have the handshake, we need to switch to the peer
            // message codec and save the socket in self (note that we need to
            // keep the buffer from the original codec as it may contain bytes
            // of any potential message the peer may have sent after the
            // handshake)
            let parts = socket.into_parts();
            let mut parts = FramedParts::new(parts.io, PeerCodec);
            // reuse buffers of previous codec
            parts.read_buf = parts.read_buf;
            parts.write_buf = parts.write_buf;
            let socket = Framed::from_parts(parts);

            // enter the piece availability exchange state until peer sends a
            // bitfield (we don't send one as we currently only implement
            // downloading so we cannot have piece availability until multiple
            // peer connections or resuming a torrent is implemented)
            self.state = State::AvailabilityExchange;
            log::info!("Peer {} session state: {:?}", self.addr, self.state);

            // run the session
            self.run(socket).await?;
        }

        Ok(())
    }

    async fn run(
        &mut self,
        mut socket: Framed<TcpStream, PeerCodec>,
    ) -> Result<()> {
        // start receiving and sending messages
        while let Some(msg) = socket.next().await {
            let msg = msg?;
            log::info!("Received message from peer {}", self.addr);
            log::debug!("Peer {} message: {:?}", self.addr, msg);

            if self.state == State::AvailabilityExchange {
                // handle bitfield message separately as it may only be received
                // directly after the handshake
                if let Message::Bitfield(bitfield) = msg {
                    // if peer is not a seed, we abort the connection as we only
                    // support downloading and for that we must be connected to
                    // a seed
                    if !bitfield.all() {
                        log::warn!(
                            "Peer {} is not a seed, cannot download",
                            self.addr
                        );
                        return Err(Error::PeerNotSeed);
                    }

                    // register peer's pieces with piece picker
                    let mut piece_picker = self.piece_picker.write().await;
                    piece_picker.register_availability(&bitfield);
                    self.status.is_interested = piece_picker.is_interested(&bitfield);

                    // enter connected state
                    //
                    // TODO: this needs to be moved out of here once we start
                    // supporting session with other leeches
                    self.state = State::Connected;
                    log::info!(
                        "Peer {} session state: {:?}",
                        self.addr,
                        self.state
                    );

                    // go to the next message (the message is consumed in this
                    // branch)
                    continue;
                } else {
                    // since we expect peer to be a seed, we *must* get
                    // a bitfield message, as otherwise we assume the peer to be
                    // a leech with no pieces to share (which is not good for
                    // our purposes of downloading a file)
                    log::warn!(
                        "Peer {} hasn't sent bitfield, cannot download",
                        self.addr
                    );
                    return Err(Error::PeerNotSeed);
                }
            }

            // handle rest of the protocol messages
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
                    log::info!("Peer {} choked us", self.addr);
                    self.status.is_choked = true;
                }
                Message::Unchoke => {
                    log::info!("Peer {} unchoked us", self.addr);
                    // TODO: here we'd set up the download pipeline future
                    self.status.is_choked = false;
                }
                Message::Interested => {
                    log::info!("Peer {} is interested", self.addr);
                    self.status.is_peer_interested = true;
                }
                Message::NotInterested => {
                    log::info!("Peer {} is not interested", self.addr);
                    self.status.is_peer_interested = false;
                }
                Message::Block {
                    piece_index,
                    offset,
                    block,
                } => {
                    log::info!(
                        "Peer {} sent piece {} block (offset {}, length {})",
                        self.addr,
                        piece_index,
                        offset,
                        block.len()
                    );
                    // TODO: here we'd save the block and mark it as downloaded
                    // in our download structures
                }
                // these messages are not expected
                //
                // TODO: decide whether to sever connection or not
                Message::Have { .. } => {
                    log::warn!(
                        "Seed {} sent unexpected message: {:?}",
                        self.addr,
                        MessageId::Have
                    );
                }
                Message::Request { .. } => {
                    log::warn!(
                        "Seed {} sent unexpected message: {:?}",
                        self.addr,
                        MessageId::Request
                    );
                }
                Message::Cancel { .. } => {
                    log::warn!(
                        "Seed {} sent unexpected message: {:?}",
                        self.addr,
                        MessageId::Cancel
                    );
                }
            }
        }

        // if we're here, it means the receiving side of the connection was
        // closed
        Ok(())
    }
}
