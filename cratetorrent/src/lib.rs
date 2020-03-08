#[macro_use]
extern crate serde_derive;

pub mod error;
pub mod metainfo;
mod peer;
mod piece_picker;
mod torrent;

use bitvec::prelude::{BitVec, Msb0};
use error::*;
use metainfo::Metainfo;
use std::net::SocketAddr;
use tokio::runtime::Runtime;
use torrent::Torrent;

pub type PeerId = [u8; 20];
pub type Sha1Hash = [u8; 20];

/// The bitfield represents the piece availability of a peer. It is a compact
/// bool vector of most significant bits to least significants bits, that is,
/// where the first highest bit represents the first piece, the second highest
/// element the second piece, and so on (e.g. `0b1100_0001` would mean that we
/// have pieces 0, 1, and 7). A truthy boolean value of a piece's position in
/// this vector means that the peer has the piece, while a falsy value means it
/// doesn't have the piece.
pub type Bitfield = BitVec<Msb0, u8>;

/// Connect to a single seed and download the torrent.
pub fn run_torrent(
    client_id: PeerId,
    metainfo: Metainfo,
    seed_addr: SocketAddr,
) -> Result<()> {
    let mut rt = Runtime::new()?;
    let mut torrent = Torrent::new(client_id, metainfo, seed_addr)?;
    let torrent_fut = torrent.start();
    rt.block_on(torrent_fut);
    Ok(())
}
