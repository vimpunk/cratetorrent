#[macro_use]
extern crate serde_derive;

pub mod error;
pub mod metainfo;
mod peer;
mod piece_picker;
mod torrent;

use error::*;
use metainfo::Metainfo;
use std::net::SocketAddr;
use torrent::Torrent;

pub type PeerId = [u8; 20];
pub type Sha1Hash = [u8; 20];

/// Connect to a single seed and download the torrent.
pub async fn run_torrent(
    client_id: PeerId,
    metainfo: Metainfo,
    seed_addr: SocketAddr,
) -> Result<()> {
    let mut torrent = Torrent::new(client_id, metainfo, seed_addr)?;
    torrent.start().await
}
