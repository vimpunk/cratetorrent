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

/// A block is a fixed size chunk of a piece, which in turn is a fixed size
/// chunk of a torrent. Downloading torrents happen at this block level
/// granularity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BlockInfo {
    /// The index of the piece of which this is a block.
    pub piece_index: u32,
    /// The byte offset into the piece.
    pub offset: u32,
    /// The length in bytes of this block. Almost always 4KiB (0x4000 bytes).
    pub length: u32
}

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
