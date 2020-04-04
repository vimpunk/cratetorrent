use {
    crate::{error::*, BlockInfo, Sha1Hash},
    sha1::{Digest, Sha1},
    tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    std::collections::HashMap,
};

// Torrent information related to disk IO.
//
// Contains the in-progress pieces (i.e. the write buffer), metadata about
// torrent's download and piece sizes, etc.
struct Torrent {
    // The channel used to notify a torrent that a block has been written to
    // disk and/or a piece was completed.
    notify_chan: Sender,
    // The in-progress piece downloads and disk writes. This is the torrent's
    // disk write buffer. Each piece is mapped to its index for faster lookups.
    //
    // TODO(https://github.com/mandreyel/cratetorrent/issues/22): Currently
    // there is no upper bound on the in-memory write buffer, so this may lead
    // to OOM.
    pieces: HashMap<usize, Piece>,
    // The concatenation of all expected piece hashes.
    piece_hashes: Vec<u8>,
    // The number of pieces in the torrent.
    piece_count: usize,
    // The nominal length of a piece.
    piece_len: u32,
    // The length of the last piece in torrent, which may differ from the normal
    // piece length if the download size is not an exact multiple of the normal
    // piece length.
    last_piece_len: u32,
    // The sum of the length of all files in the torrent.
    download_len: u64,
}

// An in-progress piece download kept that keeps in memory the so far downloaded
// blocks and a hashing context that's updated with each received block.
struct Piece {
    // The expected hash of the whole piece.
    expected_hash: Sha1Hash,
    // The length of the piece, in bytes.
    len: usize,
    // The so far downloaded blocks. Once the size of this vector reaches the
    // number of blocks in piece, the piece is complete and, if the hash is
    // correct, saved to disk.
    blocks: Vec<Vec<u8>>,
    // The continuisly updated hashing context. When a new block is received,
    // the block is hashed with this.
    //
    // NOTE:
    // Blocks have to be hashed in order as SHA-1 is a linear hash and
    // thus doesn't support out-of-order hashing!
    hasher: Sha1,
}

// The type of channel used to notify a torrent that a block was written to
// disk.
type Sender = UnboundedSender<WriteResult>;

/// The type of channel on which a torrent can listen for block write
/// completions.
pub(crate) type Receiver = UnboundedReceiver<WriteResult>;

/// Type returned on each successful block write.
pub(crate) struct WriteResult {
    /// The info of the block that was written.
    ///
    /// This is included here as there are usually multiple in-progress block
    /// writes at any given time for a download session so the future returned
    /// by the block wirte method is usually placed in some sort of container
    /// (`FuturesUnordered`), this helps to identify which block has been
    /// written to disk.
    pub info: BlockInfo,
    /// This field is set for the block write that completes the piece and
    /// contains whether the downloaded piece's hash matches its expected hash.
    pub is_piece_valid: Option<bool>,
}

/// The entity responsible for saving downloaded file blocks to disk and
/// verifying whether downloaded pieces are valid.
pub(crate) struct Disk {
    torrent: Torrent,
}

impl Disk {
    /// Creates a new `Disk` instance for a specific torrent and returns itself
    /// and a receiver on which the torrent can be notified of disk IO updates.
    pub fn new(
        piece_hashes: Vec<u8>,
        piece_count: usize,
        piece_len: u32,
        last_piece_len: u32,
        download_len: u64,
    ) -> (Self, Receiver) {
        let (notify_chan, notify_port) = unbounded_channel();
        let torrent = Torrent {
            notify_chan,
            pieces: HashMap::new(),
            piece_hashes, 
            piece_count,
            piece_len,
            last_piece_len,
            download_len,
        };
        (Self { torrent }, notify_port)
    }

    /// Queues a block for eventual saving to disk.
    ///
    /// Once the block is saved, the result is advertised to its torrent via the
    /// mpsc channel created in the `Disk` constructor.
    pub async fn save_block(
        &self,
        info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        todo!();
    }
}
