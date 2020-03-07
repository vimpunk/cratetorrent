mod codec;

use crate::error::*;
use crate::piece_picker::PiecePicker;
use crate::torrent::TorrentInfo;
use codec::*;
use futures::{SinkExt, StreamExt};
use futures_locks::RwLock;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, FramedParts};

#[derive(Clone, Copy, Debug)]
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
    state: State,
    torrent_info: Rc<TorrentInfo>,
    piece_picker: Arc<RwLock<PiecePicker>>,
    addr: SocketAddr,
    socket: Option<TcpStream>,
    status: Status,
}

impl PeerSession {
    pub fn outbound(
        torrent_info: Rc<TorrentInfo>,
        piece_picker: Arc<RwLock<PiecePicker>>,
        addr: SocketAddr,
    ) -> Self {
        Self {
            state: State::default(),
            torrent_info,
            piece_picker,
            addr,
            socket: None,
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
                return Err(Error::InvalidInfoHash);
            }

            // now that we have the handshake, we need to switch to the peer
            // message codec
            let parts = socket.into_parts();
            let mut parts = FramedParts::new(parts.io, PeerCodec);
            // reuse buffers of previous codec
            parts.read_buf = parts.read_buf;
            parts.write_buf = parts.write_buf;
            let mut socket = Framed::from_parts(parts);

            // TODO: enter the piece availability exchange state until peer
            // sends a bitfield (we don't send one as we currently only
            // implement downloading so we cannot have piece availability until
            // resuming a torrent is implemented)
            self.state = State::AvailabilityExchange;
            log::info!("Peer {} session state: {:?}", self.addr, self.state);

            self.state = State::Connected;
            log::info!("Peer {} session state: {:?}", self.addr, self.state);

            // start receiving and sending messages
            while let Some(msg) = socket.next().await {
                let msg = msg?;
                log::info!(
                    "Received message from peer {}: {:?}",
                    self.addr,
                    msg
                );
            }
        }

        Ok(())
    }
}
