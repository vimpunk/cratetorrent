use {
    crate::{error::*, BlockInfo, Sha1Hash},
    sha1::{Digest, Sha1},
};

// An in-progress piece download kept that keeps in memory the so far downloaded
// blocks and a hashing context that's updated with each received block.
struct Piece {
    // The expected hash of the whole piece.
    expected_hash: Sha1Hash,
    // The length of the piece, in bytes.
    length: usize,
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
    context: Sha1,
}

/// Type returned on each successful block write.
pub(crate) struct WriteResult {
    // The info of the block that was written.
    //
    // This is included here as there are usually multiple in-progress block
    // writes at any given time for a download session so the future returned by
    // the block wirte method is usually placed in some sort of container
    // (`FuturesUnordered`), this helps to identify which block has been written
    // to disk.
    pub info: BlockInfo,
    // This field is set for the block write that completes the piece and
    // contains whether the downloaded piece's hash matches the expected hash.
    pub is_piece_valid: Option<bool>,
}

/// The entity responsible for saving downloaded file blocks to disk and
/// verifying whether downloaded pieces are valid.
pub(crate) struct Disk {}

impl Disk {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn save_block(
        &self,
        info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<WriteResult> {
        todo!();
    }
}
