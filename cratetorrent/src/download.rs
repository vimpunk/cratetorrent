use crate::{block_count, block_len, BlockInfo, PieceIndex, BLOCK_LEN};

#[derive(Clone, Copy, Debug)]
enum Block {
    Free,
    Requested,
    Received,
}

impl Default for Block {
    fn default() -> Self {
        Self::Free
    }
}

/// Tracks the completion of an ongoing piece download and is used to request
/// missing blocks in piece.
pub(crate) struct PieceDownload {
    /// The piece's index.
    index: PieceIndex,
    /// The piece's length in bytes.
    len: u32,
    /// The blocks in this piece, tracking which are downloaded, pending, or
    /// received. The vec is preallocated to the number of blocks in piece.
    blocks: Vec<Block>,
}

impl PieceDownload {
    /// Creates a new piece download instance for the given piece.
    pub fn new(index: PieceIndex, len: u32) -> Self {
        let block_count = block_count(len);
        let mut blocks = Vec::new();
        blocks.resize_with(block_count, Default::default);
        Self { index, len, blocks }
    }

    /// Returns the index of the piece that is downloaded.
    pub fn piece_index(&self) -> PieceIndex {
        self.index
    }

    /// Picks the requested number of blocks or fewer, if fewer are remaining.
    pub fn pick_blocks(&mut self, count: usize, blocks: &mut Vec<BlockInfo>) {
        log::trace!(
            "Picking {} block(s) in piece {} (length: {}, blocks: {})",
            count,
            self.index,
            self.len,
            self.blocks.len(),
        );

        let mut picked = 0;

        for (i, block) in self.blocks.iter_mut().enumerate() {
            // don't pick more than requested
            if picked == count {
                break;
            }

            // only pick block if it's free
            if let Block::Free = block {
                blocks.push(BlockInfo {
                    piece_index: self.index,
                    offset: i as u32 * BLOCK_LEN,
                    len: block_len(self.len, i),
                });
                *block = Block::Requested;
                picked += 1;
            }

            // TODO(https://github.com/mandreyel/cratetorrent/issues/18): if we
            // requested block too long ago, time out block
        }

        if picked > 0 {
            log::debug!(
                "Picked {} block(s) for piece {}: {:?}",
                picked,
                self.index,
                &blocks[blocks.len() - picked..]
            );
        } else {
            log::debug!("Cannot pick any blocks in piece {}", self.index);
        }
    }

    /// Marks the given block as received so that it is not picked again.
    pub fn received_block(&mut self, block: &BlockInfo) {
        log::trace!("Received piece {} block {:?}", self.index, block);

        // TODO(https://github.com/mandreyel/cratetorrent/issues/16): this
        // information is sanitized in PeerSession but maybe we want to return
        // a Result anyway
        debug_assert_eq!(block.piece_index, self.index);
        debug_assert!(block.offset < self.len);
        debug_assert!(block.len <= self.len);

        // we should only receive blocks that we have requested before
        debug_assert!(matches!(
            self.blocks[block.index_in_piece()],
            Block::Requested
        ));

        self.blocks[block.index_in_piece()] = Block::Received;

        // TODO(https://github.com/mandreyel/cratetorrent/issues/9): record
        // rount trip time for this block
    }

    /// Marks a previously requested block free to request again.
    pub fn cancel_request(&mut self, block: &BlockInfo) {
        log::trace!(
            "Canceling request for piece {} block {:?}",
            self.index,
            block
        );

        // TODO(https://github.com/mandreyel/cratetorrent/issues/16): this
        // information is sanitized in PeerSession but maybe we want to return
        // a Result anyway
        debug_assert_eq!(block.piece_index, self.index);
        debug_assert!(block.offset < self.len);
        debug_assert!(block.len <= self.len);

        self.blocks[block.index_in_piece()] = Block::Free;
    }

    /// Returns true if the piece has all blocks downloaded.
    pub fn is_complete(&self) -> bool {
        self.count_missing_blocks() == 0
    }

    /// Returns the number of free (pickable) blocks.
    pub fn count_missing_blocks(&self) -> usize {
        // TODO(https://github.com/mandreyel/cratetorrent/issues/15): we could
        // optimize this by caching this value in a `count_missing_blocks` field
        // in self that is updated in pick_blocks
        self.blocks
            .iter()
            .filter(|b| matches!(b, Block::Free | Block::Requested))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    // Tests that repeatedly requesting as many blocks as are in the piece
    // returns all blocks, none of them previously picked.
    #[test]
    fn test_pick_all_blocks_one_by_one() {
        let index = 0;
        let piece_len = 6 * BLOCK_LEN;

        let mut download = PieceDownload::new(index, piece_len);

        // save picked blocks
        let block_count = block_count(piece_len);
        let mut picked = HashSet::with_capacity(block_count);

        // pick all blocks one by one
        for _ in 0..block_count {
            let mut blocks = Vec::new();
            download.pick_blocks(1, &mut blocks);
            assert_eq!(blocks.len(), 1);
            let block = *blocks.first().unwrap();
            // assert that this block hasn't been picked before
            assert!(!picked.contains(&block));
            // mark block as picked
            picked.insert(block);
        }

        // assert that we picked all blocks
        assert_eq!(picked.len(), block_count);
        for block in download.blocks.iter() {
            assert!(matches!(block, Block::Requested));
        }
    }

    // Tests that requesting as many blocks as are in the piece in one go
    // returns all blocks.
    #[test]
    fn test_pick_all_blocks() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;

        let mut download = PieceDownload::new(piece_index, piece_len);

        // pick all blocks
        let block_count = block_count(piece_len);
        let mut blocks = Vec::new();
        download.pick_blocks(block_count, &mut blocks);
        assert_eq!(blocks.len(), block_count);

        // assert that we picked all blocks
        for block in download.blocks.iter() {
            assert!(matches!(block, Block::Requested));
        }
    }

    // Tests that repeatedly requesting as many blocks as are in the piece
    // returns all blocks, none of them previously picked.
    #[test]
    fn test_receive_all_blocks() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;

        let mut download = PieceDownload::new(piece_index, piece_len);

        let block_count = block_count(piece_len);
        let mut blocks = Vec::new();
        download.pick_blocks(block_count, &mut blocks);
        assert_eq!(blocks.len(), block_count);

        // mark all blocks as requested
        for block in blocks.iter() {
            download.received_block(block);
        }

        let mut blocks = Vec::new();
        download.pick_blocks(block_count, &mut blocks);
        assert!(blocks.is_empty());
    }

    // Tests that requesting as many blocks as are in the piece in one go
    // returns only blocks not already requested or received.
    #[test]
    fn test_pick_free_blocks() {
        let piece_index = 0;
        let piece_len = 6 * BLOCK_LEN;

        let mut download = PieceDownload::new(piece_index, piece_len);

        // pick 4 blocks
        let picked_block_indices = [0, 1, 2, 3];
        let mut blocks = Vec::new();
        download.pick_blocks(picked_block_indices.len(), &mut blocks);
        assert_eq!(blocks.len(), picked_block_indices.len());

        // mark 3 of them as received
        let received_block_count = 3;
        for block in blocks.iter().take(received_block_count) {
            download.received_block(block);
        }

        let block_count = block_count(piece_len);

        assert_eq!(
            download.count_missing_blocks(),
            block_count - received_block_count
        );

        // pick all remaining free blocks
        let mut blocks = Vec::new();
        download.pick_blocks(block_count, &mut blocks);
        assert_eq!(blocks.len(), block_count - picked_block_indices.len());
    }
}
