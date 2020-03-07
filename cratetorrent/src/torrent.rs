use crate::error::*;
use crate::metainfo::Metainfo;
use crate::peer::PeerSession;
use crate::piece_picker::PiecePicker;
use crate::{PeerId, Sha1Hash};
use futures_locks::RwLock;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) struct TorrentInfo {
    // The info hash of the torrent, derived from its metainfo. This is used to
    // identify the torrent with other peers and trackers.
    pub info_hash: Sha1Hash,
    // The arbitrary client id, chosen by the user of this library. This is
    // advertised to peers and trackers.
    pub client_id: PeerId,
}

pub struct Torrent {
    // The single peer this torrent is connected to. This peer has to be a seed
    // as currently we only support downloading and no seeding.
    peer: PeerSession,
    // General information about a torrent.
    info: Rc<TorrentInfo>,
    // This is passed to peer and tracks the availability of our pieces as well
    // as pieces in the torrent swarm (more relevant when more peers are added),
    // and using this knowledge which piece to pick next.
    piece_picker: Arc<RwLock<PiecePicker>>,
}

impl Torrent {
    pub fn new(
        client_id: PeerId,
        metainfo: Metainfo,
        seed_addr: SocketAddr,
    ) -> Result<Self> {
        let info_hash = metainfo.create_info_hash()?;
        let info = Rc::new(TorrentInfo {
            info_hash,
            client_id,
        });

        let piece_picker = PiecePicker::new(metainfo.piece_count());
        let piece_picker = Arc::new(RwLock::new(piece_picker));

        let peer = PeerSession::outbound(
            info.clone(),
            Arc::clone(&piece_picker),
            seed_addr,
        );

        Ok(Self {
            peer,
            info,
            piece_picker,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        self.peer.start().await
    }
}
