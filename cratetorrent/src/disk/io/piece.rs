use std::{collections::BTreeMap, ops::Range, sync};

use sha1::{Digest, Sha1};

use crate::{
    block_count,
    disk::{error::*, io::file::TorrentFile},
    iovecs::IoVec,
    FileIndex, Sha1Hash,
};

/// An in-progress piece download that keeps in memory the so far downloaded
/// blocks and the expected hash of the piece.
pub(super) struct Piece {
    /// The expected hash of the whole piece.
    pub expected_hash: Sha1Hash,
    /// The length of the piece, in bytes.
    pub len: u32,
    /// The so far downloaded blocks. Once the size of this map reaches the
    /// number of blocks in piece, the piece is complete and, if the hash is
    /// correct, saved to disk.
    ///
    /// Each block must be 16 KiB and is mapped to its offset within piece. A
    /// BTreeMap is used to keep blocks sorted by their offsets, which is
    /// important when iterating over the map to hash each block in the right
    /// order.
    // TODO: consider whether using a preallocated Vec of Options would be more
    // performant due to cache locality (we would have to count the missing
    // blocks though, or keep a separate counter)
    pub blocks: BTreeMap<u32, Vec<u8>>,
    /// The files that this piece overlaps with.
    ///
    /// This is a left-inclusive range of all all file indices, that can be used
    /// to index the `Torrent::files` vector to get the file handles.
    pub file_range: Range<FileIndex>,
}

impl Piece {
    /// Places block into piece's write buffer if it doesn't exist. TODO: should
    /// we return an error if it does?
    pub fn enqueue_block(&mut self, offset: u32, data: Vec<u8>) {
        if self.blocks.contains_key(&offset) {
            log::warn!("Duplicate piece block at offset {}", offset);
        } else {
            self.blocks.insert(offset, data);
        }
    }

    /// Returns true if the piece has all its blocks in its write buffer.
    pub fn is_complete(&self) -> bool {
        self.blocks.len() == block_count(self.len)
    }

    /// Calculates the piece's hash using all its blocks and returns if it
    /// matches the expected hash.
    ///
    /// # Important
    ///
    /// This is potentially a computationally expensive function and should be
    /// executed on a thread pool and not the executor.
    pub fn matches_hash(&self) -> bool {
        // sanity check that we only call this method if we have all blocks in
        // piece
        debug_assert_eq!(self.blocks.len(), block_count(self.len));
        let mut hasher = Sha1::new();
        for block in self.blocks.values() {
            hasher.input(&block);
        }
        let hash = hasher.result();
        log::debug!("Piece hash: {:x}", hash);
        hash.as_slice() == self.expected_hash
    }

    /// Writes the piece's blocks to the files the piece overlaps with.
    ///
    /// # Important
    ///
    /// This performs sync IO and is thus potentially blocking and should be
    /// executed on a thread pool, and not the async executor.
    pub fn write(
        &self,
        torrent_piece_offset: u64,
        files: &[sync::RwLock<TorrentFile>],
    ) -> Result<(), WriteError> {
        // convert the blocks to IO slices that the underlying
        // systemcall can deal with
        let mut blocks: Vec<_> = self
            .blocks
            .values()
            .map(|b| IoVec::from_slice(&b))
            .collect();
        // the actual slice of blocks being worked on
        let mut bufs = blocks.as_mut_slice();

        // loop through all files piece overlaps with and write that part of
        // piece to file
        let files = &files[self.file_range.clone()];
        debug_assert!(!files.is_empty());
        // the offset at which we need to write in torrent, which is updated
        // with each write
        let mut torrent_write_offset = torrent_piece_offset;
        let mut total_write_count = 0;

        for file in files.iter() {
            let file = file.write().unwrap();

            // determine which part of the file we need to write to
            debug_assert!(self.len as u64 > total_write_count);
            let remaining_piece_len = self.len as u64 - total_write_count;
            let file_slice = file
                .info
                .get_slice(torrent_write_offset, remaining_piece_len);
            // an empty file slice shouldn't occur as it would mean that piece
            // was thought to span fewer files than it actually does
            debug_assert!(file_slice.len > 0);
            // the write buffer should still contain bytes to write
            debug_assert!(!bufs.is_empty());
            debug_assert!(!bufs[0].as_slice().is_empty());

            // write to file
            let tail = file.write(file_slice, bufs)?;

            // `write_vectored_at` only writes at most `slice.len` bytes of
            // `bufs` to disk and returns the portion that wasn't
            // written, which we can use to set the write buffer for the next
            // round
            bufs = tail;

            torrent_write_offset += file_slice.len as u64;
            total_write_count += file_slice.len;
        }

        // we should have used up all write buffers (i.e. written all blocks to
        // disk)
        debug_assert!(bufs.is_empty());

        Ok(())
    }
}
