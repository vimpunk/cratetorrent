use {
    crate::{block_count, error::*, BlockInfo, Sha1Hash},
    sha1::{Digest, Sha1},
    std::collections::{BTreeMap, HashMap},
    tokio::{
        sync::{
            mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
            RwLock,
        },
        task,
    },
};

// The type of channel used to notify a torrent that a block was written to
// disk.
type Sender = UnboundedSender<WriteResult>;

/// The type of channel on which a torrent can listen for block write
/// completions.
pub(crate) type Receiver = UnboundedReceiver<WriteResult>;

/// Type returned on each successful block write.
#[derive(Debug)]
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
    torrent: RwLock<Torrent>,
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
        let torrent = RwLock::new(torrent);
        (Self { torrent }, notify_port)
    }

    /// Queues a block for eventual saving to disk.
    ///
    /// Once the block is saved, the result is advertised to its torrent via the
    /// mpsc channel created in the `Disk` constructor.
    ///
    /// The function returns as soon as the block could be queued onto the
    /// torrent's disk write buffer, and _not_ when the block was written to
    /// disk.
    pub async fn save_block(
        &self,
        info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        log::trace!("Saving block {:?} to disk", info);
        self.torrent.write().await.save_block(info, data)
    }
}

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

impl Torrent {
    fn save_block(&mut self, info: BlockInfo, data: Vec<u8>) -> Result<()> {
        if !self.pieces.contains_key(&info.piece_index) {
            self.start_new_piece(info)?;
        };
        // TODO: don't unwrap here
        let piece = self.pieces.get_mut(&info.piece_index).unwrap();

        piece.enqueue_block(info.offset, data);

        // if the piece has all its blocks, so we can finish the hash, save it
        // to disk, and remove it from memory
        if piece.is_complete() {
            // TODO: remove from in memory store only if the disk write
            // succeeded (otherwise we need to retry later)
            let piece = self.pieces.remove(&info.piece_index).unwrap();

            let notify_chan = self.notify_chan.clone();

            // don't block the reactor with the potentially expensive hashing
            task::spawn_blocking(move || {
                let mut hasher = Sha1::new();
                for block in piece.blocks.values() {
                    hasher.input(&block);
                }
                let hash = hasher.result();

                let expected_hash = piece.expected_hash;

                // TODO: save piece to disk

                let blocks = piece.blocks.values();

                // notify torrent of each block before the last one that has
                // been written to disk
                for (offset, _) in blocks.take(piece.blocks.len() - 1).enumerate() {
                    let info = BlockInfo::new(info.piece_index, offset as u32);
                    notify_chan
                        .send(WriteResult {
                            info,
                            is_piece_valid: None,
                        })
                        .map_err(|e| {
                            log::error!(
                                "Error sending disk notification to torrent: {}",
                                e
                            );
                            Error::Disk
                        })?;
                }

                // Finally, notify torrent of the last block in piece that also
                // contains the piece result. This is so that torrent need only
                // handle a piece completion event once, but later we should
                // make this more robust as later piece completions may not
                // always be in order.
                let is_piece_valid = Some(hash.as_slice() == expected_hash);
                notify_chan
                    .send(WriteResult {
                        info,
                        is_piece_valid,
                    })
                    .map_err(|e| {
                        log::error!(
                            "Error sending disk notification to torrent: {}",
                            e
                        );
                        Error::Disk
                    })
            });
        }

        Ok(())
    }

    fn start_new_piece(&mut self, info: BlockInfo) -> Result<()> {
        log::trace!("Creating piece {} write buffer", info.piece_index);

        // get the position of the piece in the concatenated hash string
        let hash_pos = info.piece_index * 20;
        if hash_pos + 20 > self.piece_hashes.len() {
            return Err(Error::InvalidPieceIndex);
        }

        let hash_slice = &self.piece_hashes[hash_pos..hash_pos + 20];
        let mut expected_hash = [0; 20];
        expected_hash.copy_from_slice(hash_slice);

        let len = if info.piece_index == self.piece_count - 1 {
            self.last_piece_len
        } else {
            self.piece_len
        };

        let blocks = BTreeMap::new();

        let piece = Piece {
            expected_hash,
            len,
            blocks,
        };
        self.pieces.insert(info.piece_index, piece);

        Ok(())
    }
}

// An in-progress piece download kept that keeps in memory the so far downloaded
// blocks and a hashing context that's updated with each received block.
struct Piece {
    // The expected hash of the whole piece.
    expected_hash: Sha1Hash,
    // The length of the piece, in bytes.
    len: u32,
    // The so far downloaded blocks. Once the size of this map reaches the
    // number of blocks in piece, the piece is complete and, if the hash is
    // correct, saved to disk.
    //
    // Each block must be 4 KiB and is mapped to its offset within piece, and
    // we're using a BTreeMap to keep keys sorted. This is important when
    // iterating over the map to hash each block after one another.
    blocks: BTreeMap<u32, Vec<u8>>,
}

impl Piece {
    fn enqueue_block(&mut self, offset: u32, data: Vec<u8>) {
        if self.blocks.contains_key(&offset) {
            log::warn!("Duplicate piece block at offset {}", offset);
        } else {
            self.blocks.insert(offset, data);
        }
    }

    fn is_complete(&self) -> bool {
        self.blocks.len() == block_count(self.len)
    }
}
