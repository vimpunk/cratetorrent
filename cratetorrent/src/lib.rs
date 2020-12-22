//! `cratetorrent` is a peer-to-peer file-sharing engine implementing the
//! BitTorrent version 1 protocol.
//!
//! It is built on top of [`tokio`](https://docs.rs/tokio/0.2.16/tokio/) for
//! async IO.
//!
//! # Caveats
//!
//! The engine currently only supports Linux. This is expected to change in the
//! future, however.
//!
//! It also lacks most features present in battle-hardened torrent engines, such
//! as [libtorrent](https://github.com/arvidn/libtorrent). These include: DHT
//! for peer exchange, magnet links, stream encryption, UDP trackers, and many
//! more.
//!
//! Therefore in the current state of the project, this should only be viewed as
//! a toy program.
//!
//! # The engine
//!
//! Users of the library will be interacting with the [engine](crate::engine),
//! which drives uploads and downloads (in short, torrents).
//!
//! As a first step of using cratetorrent, the engine needs to be spawned and
//! started. This must happen within a tokio context--that is, on a task
//! already spawned on the tokio executor. A useful macro for this is
//! [`tokio::main`](https://docs.rs/tokio/0.2.16/tokio/attr.main.html).
//!
//! The engine may be configured via the types in the [`conf`] module, though
//! options at the moment are fairly limited.
//!
//! # Downloading
//!
//! To start a download, as decribed above, the cratetorrent engine has to be
//! already running.
//!
//! Then, the [metainfo](https://en.wikipedia.org/wiki/Torrent_file) of the
//! torrent needs to be constructed. This, as its name suggests, contains
//! metadata about the torrent. It will be most commonly be downloaded as a file
//! from a torrent tracker.
//!
//! This then can be read in as a file and parsed into
//! a [`Metainfo`](crate::metainfo::Metainfo) instance using its
//! [constructor](crate::metainfo::Metainfo::from_bytes). This will fail if the
//! metainfo is semantically or syntactically invalid.
//!
//! Note that in order to download a torrent the metainfo has to contain HTTP
//! trackers, or some seeds have to be manually specified. As mentioned above,
//! DHT or even UDP trackers are not currently supported.
//!
//! Once this is done, a command to the engine has to be sent to create the
//! torrent. This is done using
//! [`EngineHandle::create_torrent`](crate::engine::EngineHandle::create_torrent),
//! which takes a [`TorrentParams`](crate::engine::TorrentParams) instance for
//! the torrent's parameters.
//!
//! Also included is an option to override the global torrent configuration
//! (passed to engine, as mentioned above) via
//! [`TorrentConf`](crate::conf::TorrentConf). If not set, the global
//! configuration is used for all new torrents, but this way it is possible to
//! configure a torrent on a case-by-case basis.
//!
//! For now, the torrent's download mode has to be specified via `Mode`:
//! whether to download or seed (upload) the torrent. If the latter is chosen,
//! the torrent's contents _have_ to exist in the directory specified as the
//! torrent's download directory in `TorrentConf`.
//!
//! ## Full example of a download
//!
//! An example download of an arbitrary torrent download that exits a soon as
//! the download is complete might look like this:
//!
//! ```no_run
//! use cratetorrent::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // spawn the engine with a default config
//!     let conf = Conf::new("/tmp/downloads");
//!     let (engine, mut alert_rx) = engine::spawn(conf)?;
//!
//!     // parse torrent metainfo and start the download (use tokio::fs)
//!     let metainfo = std::fs::read("/tmp/imaginary.torrent")?;
//!     let metainfo = Metainfo::from_bytes(&metainfo)?;
//!     let torrent_id = engine.create_torrent(TorrentParams {
//!         metainfo,
//!         // tell the engine to assign a randomly chosen free port
//!         listen_addr: None,
//!         mode: Mode::Download { seeds: Vec::new() },
//!         conf: None,
//!     })?;
//!
//!     // listen to alerts from the engine
//!     while let Some(alert) = alert_rx.next().await {
//!         match alert {
//!             Alert::TorrentStats { id, stats } => {
//!                 println!("{}: {:#?}", id, stats);
//!             }
//!             Alert::TorrentComplete(id) => {
//!                 println!("{} complete, shutting down", id);
//!                 break;
//!             }
//!             Alert::Error(e) => {
//!                 // this is where you'd handle recoverable errors
//!                 println!("Engine error: {}", e);
//!             }
//!             _ => (),
//!         }
//!     }
//!
//!     // Don't forget to call shutdown on the engine to gracefully stop all
//!     // entities in the engine. This will wait for announcing the client's
//!     // leave to all trackers of torrent, finish pending disk and network IO,
//!     // as well as wait for peer connections to cleanly shut down.
//!     engine.shutdown().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Seeding
//!
//! Seeding is fairly analogous to the above.
//!
//! The major differences are that the torrent mode has to be specified as
//! [`Mode::Seed`](crate::engine::Mode::Seed) and that the engine won't send
//! a notification of completion, as the concept is not applicable to
//! seeding--it's indefinite until the user stops it.
//!
//! Therefore the application must make sure to provide its own way of stopping
//! the download.

// needed by the `select!` macro reaching the default recursion limit
#![recursion_limit = "256"]

#[macro_use]
extern crate serde_derive;

use std::{
    fmt,
    ops::Deref,
    sync::atomic::{AtomicU32, Ordering},
    sync::Arc,
};

use bitvec::prelude::{BitVec, Msb0};

pub use storage_info::FileInfo;

pub mod alert;
mod avg;
pub mod conf;
mod counter;
mod disk;
mod download;
pub mod engine;
pub mod error;
pub mod iovecs;
pub mod metainfo;
pub mod peer;
mod piece_picker;
pub mod prelude;
pub mod storage_info;
pub mod torrent;
mod tracker;

/// Each torrent gets a randomly assigned ID that is unique within the
/// engine. This id is used in engine APIs to interact with torrents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct TorrentId(u32);

impl TorrentId {
    /// Produces a new unique torrent id per process.
    pub(crate) fn new() -> Self {
        lazy_static::lazy_static! {
            static ref TORRENT_ID: AtomicU32 = AtomicU32::new(0);
        }

        // the atomic is not used to synchronize data access around it so
        // relaxed ordering is fine for our purposes
        let id = TORRENT_ID.fetch_add(1, Ordering::Relaxed);
        Self(id)
    }
}

impl fmt::Display for TorrentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t#{}", self.0)
    }
}

/// The type of a file's index.
pub(crate) type FileIndex = usize;

/// The type of a piece's index.
///
/// On the wire all integers are sent as 4-byte big endian integers, but in the
/// source code we use `usize` to be consistent with other index types in Rust.
pub(crate) type PieceIndex = usize;

/// The peer ID is an arbitrary 20 byte string.
///
/// Guidelines for choosing a peer ID: http://bittorrent.org/beps/bep_0020.html.
pub type PeerId = [u8; 20];

/// A SHA-1 hash digest, 20 bytes long.
pub type Sha1Hash = [u8; 20];

/// The bitfield represents the piece availability of a peer.
///
/// It is a compact bool vector of most significant bits to least significants
/// bits, that is, where the first highest bit represents the first piece, the
/// second highest element the second piece, and so on (e.g. `0b1100_0001` would
/// mean that we have pieces 0, 1, and 7). A truthy boolean value of a piece's
/// position in this vector means that the peer has the piece, while a falsy
/// value means it doesn't have the piece.
pub type Bitfield = BitVec<Msb0, u8>;

/// Whether a torrent or peer connection is a seed or a leech.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Leech,
    Seed,
}

impl Default for Side {
    fn default() -> Self {
        Self::Leech
    }
}

/// This is the only block length we're dealing with (except for possibly the
/// last block).  It is the widely used and accepted 16 KiB.
pub(crate) const BLOCK_LEN: u32 = 0x4000;

/// A block is a fixed size chunk of a piece, which in turn is a fixed size
/// chunk of a torrent. Downloading torrents happen at this block level
/// granularity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct BlockInfo {
    /// The index of the piece of which this is a block.
    pub piece_index: PieceIndex,
    /// The zero-based byte offset into the piece.
    pub offset: u32,
    /// The block's length in bytes. Always 16 KiB (0x4000 bytes) or less, for
    /// now.
    pub len: u32,
}

impl BlockInfo {
    /// Returns the index of the block within its piece, assuming the default
    /// block length of 16 KiB.
    pub fn index_in_piece(&self) -> usize {
        // we need to use "lower than or equal" as this may be the last block in
        // which case it may be shorter than the default block length
        debug_assert!(self.len <= BLOCK_LEN);
        debug_assert!(self.len > 0);
        (self.offset / BLOCK_LEN) as usize
    }
}

impl fmt::Display for BlockInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(piece: {} offset: {} len: {})",
            self.piece_index, self.offset, self.len
        )
    }
}

/// Returns the length of the block at the index in piece.
///
/// If the piece is not a multiple of the default block length, the returned
/// value is smalled.
///
/// # Panics
///
/// Panics if the index multiplied by the default block length would exceed the
/// piece length.
pub(crate) fn block_len(piece_len: u32, block_index: usize) -> u32 {
    let block_index = block_index as u32;
    let block_offset = block_index * BLOCK_LEN;
    assert!(piece_len > block_offset);
    std::cmp::min(piece_len - block_offset, BLOCK_LEN)
}

/// Returns the number of blocks in a piece of the given length.
pub(crate) fn block_count(piece_len: u32) -> usize {
    // all but the last piece are a multiple of the block length, but the
    // last piece may be shorter so we need to account for this by rounding
    // up before dividing to get the number of blocks in piece
    (piece_len as usize + (BLOCK_LEN as usize - 1)) / BLOCK_LEN as usize
}

/// A piece block that contains the block's metadata and data.
pub(crate) struct Block {
    /// The index of the piece of which this is a block.
    pub piece_index: PieceIndex,
    /// The zero-based byte offset into the piece.
    pub offset: u32,
    /// The actual raw data of the block.
    pub data: BlockData,
}

impl Block {
    /// Constructs a new block based on the metadata and data.
    pub fn new(info: BlockInfo, data: impl Into<BlockData>) -> Self {
        Self {
            piece_index: info.piece_index,
            offset: info.offset,
            data: data.into(),
        }
    }

    /// Returns a [`BlockInfo`] representing the metadata of this block.
    pub fn info(&self) -> BlockInfo {
        BlockInfo {
            piece_index: self.piece_index,
            offset: self.offset,
            len: self.data.len() as u32,
        }
    }
}

/// Abstracts over the block data type.
///
/// A block may be just a normal byte buffer, or it may be a reference into
/// a cache.
#[derive(Debug, PartialEq)]
#[cfg_attr(test, derive(Clone))]
pub(crate) enum BlockData {
    Owned(Vec<u8>),
    Cached(CachedBlock),
}

/// Blocks are cached in memory and are shared between the disk task and peer
/// session tasks. Therefore we use atomic reference counting to make sure that
/// even if a block is evicted from cache, the peer still using it still has
/// a valid reference to it.
pub(crate) type CachedBlock = Arc<Vec<u8>>;

impl BlockData {
    /// Returns the raw block if it's owned.
    ///
    /// # Panics
    ///
    /// This method panics if the block is not owned and is in the cache.
    pub fn into_owned(self) -> Vec<u8> {
        match self {
            Self::Owned(b) => b,
            _ => panic!("cannot move block out of cache"),
        }
    }
}

impl Deref for BlockData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(b) => b.as_ref(),
            Self::Cached(b) => b.as_ref(),
        }
    }
}

impl From<Vec<u8>> for BlockData {
    fn from(b: Vec<u8>) -> Self {
        Self::Owned(b)
    }
}

impl From<CachedBlock> for BlockData {
    fn from(b: CachedBlock) -> Self {
        Self::Cached(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An arbitrary piece length that is an exact multiple of the canonical
    // block length (16 KiB).
    const BLOCK_LEN_MULTIPLE_PIECE_LEN: u32 = 2 * BLOCK_LEN;

    // An arbitrary piece length that is _not_ a multiple of the canonical block
    // length and the amount with which it overlaps the nearest exact multiple
    // value.
    const OVERLAP: u32 = 234;
    const UNEVEN_PIECE_LEN: u32 = 2 * BLOCK_LEN + OVERLAP;

    #[test]
    fn test_block_len() {
        assert_eq!(block_len(BLOCK_LEN_MULTIPLE_PIECE_LEN, 0), BLOCK_LEN);
        assert_eq!(block_len(BLOCK_LEN_MULTIPLE_PIECE_LEN, 1), BLOCK_LEN);

        assert_eq!(block_len(UNEVEN_PIECE_LEN, 0), BLOCK_LEN);
        assert_eq!(block_len(UNEVEN_PIECE_LEN, 1), BLOCK_LEN);
        assert_eq!(block_len(UNEVEN_PIECE_LEN, 2), OVERLAP);
    }

    #[test]
    #[should_panic]
    fn test_block_len_invalid_index_panic() {
        block_len(BLOCK_LEN_MULTIPLE_PIECE_LEN, 2);
    }

    #[test]
    fn test_block_count() {
        assert_eq!(block_count(BLOCK_LEN_MULTIPLE_PIECE_LEN), 2);

        assert_eq!(block_count(UNEVEN_PIECE_LEN), 3);
    }
}
