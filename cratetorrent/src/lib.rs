#[macro_use]
extern crate serde_derive;

pub mod metainfo;
mod peer;
mod piece_picker;
mod torrent;

use metainfo::*;
use peer::*;
use torrent::Torrent;
use std::net::SocketAddr;
use tokio::io::Error;

pub type PeerId = [u8; 20];
pub type Sha1Hash = [u8; 20];

pub async fn run_torrent(
    client_id: PeerId,
    metainfo: Metainfo,
    seed_addr: SocketAddr,
) -> Result<(), Error> {
    let mut torrent = Torrent::new(client_id, metainfo, seed_addr);
    torrent.start().await
}
