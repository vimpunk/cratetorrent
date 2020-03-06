#[macro_use]
extern crate serde_derive;

pub mod metainfo;
mod peer;

use metainfo::*;
use peer::*;
use std::net::SocketAddr;
use tokio::io::Error;

pub type PeerId = [u8; 20];
pub type Sha1Hash = [u8; 20];

pub async fn connect_to_peer(
    client_id: PeerId,
    addr: SocketAddr,
    metainfo: Metainfo,
) -> Result<(), Error> {
    // TODO: don't unwrap
    let info_hash = metainfo.create_info_hash().unwrap();
    let mut peer = PeerSession::outbound(addr, client_id, info_hash);
    peer.start().await
}
