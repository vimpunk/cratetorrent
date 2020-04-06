#[macro_use]
extern crate serde_derive;

mod disk;
mod download;
pub mod error;
pub mod metainfo;
mod peer;
mod piece_picker;
mod torrent;

use {
    bitvec::prelude::{BitVec, Msb0},
    error::*,
    metainfo::Metainfo,
    std::{net::SocketAddr, path::Path},
    tokio::runtime::Runtime,
    torrent::Torrent,
};

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

/// This is the only block length we're dealing with. It is the widely used and
/// accepted 4KiB.
pub(crate) const BLOCK_LEN: u32 = 0x4000;

/// A block is a fixed size chunk of a piece, which in turn is a fixed size
/// chunk of a torrent. Downloading torrents happen at this block level
/// granularity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct BlockInfo {
    /// The index of the piece of which this is a block.
    pub piece_index: usize,
    /// The zero-based byte offset into the piece.
    pub offset: u32,
    /// The block's length in bytes. Always 4KiB (0x4000 bytes), for now.
    pub len: u32,
}

impl BlockInfo {
    /// Createas a `BlockInfo` instance with the default length of 4 KiB.
    pub fn new(piece_index: usize, offset: u32) -> Self {
        Self {
            piece_index,
            offset,
            len: BLOCK_LEN,
        }
    }

    /// Returns the index of the block within its piece.
    pub fn index(&self) -> usize {
        self.offset as usize / self.len as usize
    }
}

/// Returns the number of blocks in a piece of the given length.
pub(crate) fn block_count(piece_len: u32) -> usize {
    // all but the last piece are a multiple of the block length, but the
    // last piece may be shorter so we need to account for this by rounding
    // up before dividing to get the number of blocks in piece
    (piece_len as usize + (BLOCK_LEN as usize - 1)) / BLOCK_LEN as usize
}

/// Connects to a single seed and downloads the torrent or aborts on error.
pub fn run_torrent(
    client_id: PeerId,
    download_dir: &Path,
    metainfo: Metainfo,
    seed_addr: SocketAddr,
) -> Result<()> {
    let mut rt = Runtime::new()?;
    let mut torrent =
        Torrent::new(download_dir, client_id, metainfo, seed_addr)?;
    let torrent_fut = torrent.start();
    rt.block_on(torrent_fut);
    Ok(())
}
