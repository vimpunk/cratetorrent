use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use crate::{error::*, metainfo::Metainfo, FileIndex, PieceIndex};

pub use nix::sys::uio::IoVec;

/// Information about a torrent's file.
#[derive(Clone, Debug)]
pub struct FileInfo {
    /// The file's relative path from the download directory.
    pub path: PathBuf,
    /// The file's length, in bytes.
    pub len: u64,
    /// The byte offset of the file within the torrent, when all files in
    /// torrent are viewed as a single contiguous byte array. This is always
    /// 0 for a single file torrent.
    pub torrent_offset: u64,
}

impl FileInfo {
    /// Returns a range that represents the file's first and one past the last
    /// bytes' offsets in the torrent.
    pub(crate) fn byte_range(&self) -> Range<u64> {
        self.torrent_offset..self.torrent_end_offset()
    }

    /// Returns the file's one past the last byte's offset in the torrent.
    pub(crate) fn torrent_end_offset(&self) -> u64 {
        self.torrent_offset + self.len
    }

    /// Returns the slice in file that overlaps with the range starting at the
    /// given offset.
    ///
    /// # Arguments
    ///
    /// * `torrent_offset` - A byte offset in the entire torrent.
    /// * `len` - The length of the byte range, starting from the offset. This
    ///         may be exceed the file length, in which case the returned file
    ///         length will be smaller.
    ///
    /// # Panics
    ///
    /// This will panic if `torrent_offset` is smaller than the file's offset in
    /// torrent, or if it's past the last byte in file.
    pub(crate) fn get_slice(&self, torrent_offset: u64, len: u64) -> FileSlice {
        if torrent_offset < self.torrent_offset {
            panic!("torrent offset must be larger than file offset");
        }

        let torrent_end_offset = self.torrent_end_offset();
        if torrent_offset >= torrent_end_offset {
            panic!("torrent offset must be smaller than file end offset");
        }

        FileSlice {
            offset: torrent_offset - self.torrent_offset,
            len: len.min(torrent_end_offset - torrent_offset),
        }
    }
}

/// Represents the location of a range of bytes within a file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FileSlice {
    /// The byte offset in file, relative to the file's start.
    pub offset: u64,
    /// The length of the slice, in bytes.
    pub len: u64,
}

impl FileSlice {
    /// Splits the slice of iovecs into two, at the file boundary.
    ///
    /// If the total size of the buffers exceeds the file slice's length, the
    /// buffers are split such that the first half returned is the portion of
    /// the buffers that can be written to the file, while the second half is
    /// the remainder of the buffer, to be reused later. If the size of the
    /// total size of the buffers is smaller than or equal to the slice, this is
    /// essentially a noop.
    ///
    /// Visualized, this looks like the following:
    ///
    /// ------------------------------
    /// | file slice: 25             |
    /// -----------------------------------
    /// | block: 16      | block: 16 ^    |
    /// -----------------------------^-----
    ///                              ^
    ///                          split here
    ///
    /// In this example, the first half of the split would be [0, 25), the
    /// second half would be [25, 48).
    ///
    /// # Important
    ///
    /// In reality, the situation here is more complex than just splitting
    /// buffers in half, but this is taken care of by the [`IoVecSplit`]
    /// implementation.
    ///
    /// However, the abstraction leaks through because the iovec at which the
    /// buffers were split may be shrunk to such a size as would enable all
    /// buffers to stay within the file slice length. This can be restored using
    /// [`IoVecSplit::into_remainder`], but until this is called, the original
    /// buffers cannot be used, which is enforced by the borrow checker.
    pub fn split_bufs_at_boundary<'a>(
        &self,
        bufs: &'a mut [IoVec<&'a [u8]>],
    ) -> IoVecSplit<'a> {
        // Detect whether the total byte count in bufs exceeds the slice length
        // by accumulating the buffer lengths and stopping at the buffer whose
        // accumulated length exceeds the slice length. Taking the example in
        // the docs, this would mean that we stop at the second buffer.
        //
        // i | len | cond
        // --|-----|---------------
        // 0 | 16  | len >= 25 -> f
        // 1 | 32  | len >= 32 -> t
        //
        // Another edge case is that the file boundary is at a buffer boundary.
        //
        // -----------------------------------
        // | file slice: 32                  |
        // ----------------------------------------------------
        // | block: 16      | block: 16      | block: 16      |
        // ----------------------------------^-----------------
        //                                   ^
        //                               split here
        //
        // i | len | cond
        // --|-----|---------------
        // 0 | 16  | len >= 32 -> f
        // 1 | 32  | len >= 32 -> t
        //
        // TODO: Can we make use of the fact that blocks are of the same length,
        // except for potentially the last one? We could skip over the first
        // n blocks whose summed up size is still smaller than the file .
        let mut bufs_len = 0;
        let bufs_split_pos = match bufs.iter().position(|buf| {
            bufs_len += buf.as_slice().len() as u64;
            bufs_len >= self.len
        }) {
            Some(pos) => pos,
            None => return IoVecSplit::no_split(bufs),
        };

        // If we're here, it means that the total buffers length exceeds the
        // slice length and we must split the buffers.
        if bufs_len == self.len {
            // The buffer boundary aligns with the file boundary. There are two
            // cases here:
            // 1. the buffers are the same length as the file, in which case
            //    there is nothing to split,
            // 2. or we just need to split at the buffer boundary.
            if bufs_split_pos + 1 == bufs.len() {
                // the split position is the end of the last buffer, so there is
                // nothing to split
                IoVecSplit::no_split(bufs)
            } else {
                // we can split at the buffer boundary
                IoVecSplit::split_at_boundary(bufs, bufs_split_pos)
            }
        } else {
            // Otherwise the buffer boundary does not align with the file
            // boundary (as in the doc example), so we must trim the iovec that
            // is at the file boundary.

            // Find the position where we need to split the iovec. We need the
            // relative offset of the buffer from its start, which we can get by
            // first getting the absolute offset of the buffer from the start of
            // all buffers, and then subtracting that from the file length.
            let buf_to_split = bufs[bufs_split_pos].as_slice();
            let buf_offset = bufs_len - buf_to_split.len() as u64;
            let buf_split_pos = (self.len - buf_offset) as usize;
            debug_assert!(buf_split_pos < buf_to_split.len());

            IoVecSplit::split_within_buffer(bufs, bufs_split_pos, buf_split_pos)
        }
    }
}

/// Splits the slice into two at the position, including the element at the
/// position in the first half of the split.
fn split_at_mut_inclusive<'a, T>(
    slice: &'a mut [T],
    pos: usize,
) -> (&'a mut [T], &'a mut [T]) {
    // Add one to the split position because `split_at` family of methods works
    // such that the element at the split point is not included in the first
    // half, which we want to include.
    slice.split_at_mut(pos + 1)
}

/// Represents a split of an `&mut [IoVec<&[u8]>]` into two, where the split may
/// not be on the boundary of two buffers.
///
/// The complication arises from the fact that the split may not be on a buffer
/// boundary, but we want to perform the split by keeping the original
/// slices (i.e. without allocating a new vector). This requires keeping the
/// first part of the slice, the second part of the slice, and if the split
/// occurred within a buffer, a copy of the second half of that split buffer.
///
/// This way, the user can use the first half of the buffers to pass it for
/// vectored IO, without accidentally extending the file if it's too large, and
/// then reconstructing the second half of the split, using the copy of the
/// split buffer.
///
/// Only [`FileSlice::split_bufs_at_boundary`] can construct this type, due to
/// usage of unsafe code.
#[derive(Debug)]
pub(crate) struct IoVecSplit<'a> {
    /// The entire view of the buffer.
    bufs: &'a mut [IoVec<&'a [u8]>],
    /// If not set, the buffer doesn't need to be split.
    second_half: Option<SplitSecondHalf>,
}

#[derive(Debug)]
struct SplitSecondHalf {
    /// The position at which to split `IoVecSplit::bufs`. Note that this is
    /// inclusive, i.e. we split at `[0, pos]`.
    pos: usize,
    /// If set, it means that the buffer at `bufs[split_pos]` was further split
    /// in two. It contains the second half of the split buffer.
    split_buf_second_half: Option<RawBuf>,
}

#[derive(Debug)]
struct RawBuf {
    ptr: *const u8,
    len: usize,
}

impl<'a> IoVecSplit<'a> {
    /// Creates a "full left split", when the second half of the split would be
    /// empty.
    fn no_split(bufs: &'a mut [IoVec<&'a [u8]>]) -> Self {
        Self {
            bufs,
            second_half: None,
        }
    }

    /// Creates a "clean split", when the split occurs at the buffer boundary
    /// and `bufs` need only be split at the slice level.
    fn split_at_boundary(bufs: &'a mut [IoVec<&'a [u8]>], pos: usize) -> Self {
        Self {
            bufs,
            second_half: Some(SplitSecondHalf {
                pos,
                split_buf_second_half: None,
            }),
        }
    }

    /// Creates a split where the split occurs within one of the buffers of
    /// `bufs`.
    ///
    /// TODO: we can calculate `buf_split_pos` inside this function
    fn split_within_buffer(
        bufs: &'a mut [IoVec<&'a [u8]>],
        split_pos: usize,
        buf_split_pos: usize,
    ) -> Self {
        // save the original slice at the boundary, so that later we can
        // restore it
        let buf_to_split = bufs[split_pos].as_slice();

        // trim the overhanging part off the iovec
        let (split_buf_first_half, split_buf_second_half) =
            buf_to_split.split_at(buf_split_pos);

        // We need to convert the second half of the split buffer into its
        // raw representation, as we can't store a reference to it as well
        // as store mutable references to the rest of the buffer in
        // `IoVecSplit`. This, however, is also safe, as we don't leak the
        // raw buffer and pointers for other code to unsafely reconstruct
        // the slice. The slice is only reconstructined in
        // `IoVecSplit::into_second_half`, assigning it to the `IoVec` at
        // `split_pos` in `bufs`, without touching its underlying memory.
        let split_buf_second_half = RawBuf {
            ptr: split_buf_second_half.as_ptr(),
            len: split_buf_second_half.len(),
        };

        // Shrink the iovec at the file boundary:
        //
        // Here we need to use unsafe code as there is no way to borrow
        // a slice from `bufs` (`buf_to_split` above), and then assigning
        // that same slice to another element of bufs below, as that would
        // be an immutable and mutable borrow at the same time, breaking
        // aliasing rules.
        // (https://doc.rust-lang.org/std/primitive.slice.html#method.swap
        // doesn't work here as we're not simply swapping elements of within
        // the same slice.)
        //
        // However, it is safe to do so, as we're not actually touching the
        // underlying byte buffer that the slice refers to, but simply replacing
        // the `IoVec` at `split_pos` in `bufs`, i.e.  shrinking the slice
        // itself, not the memory region pointed to by the slice.
        let split_buf_first_half = unsafe {
            std::slice::from_raw_parts(
                split_buf_first_half.as_ptr(),
                split_buf_first_half.len(),
            )
        };
        std::mem::replace(
            &mut bufs[split_pos],
            IoVec::from_slice(split_buf_first_half),
        );

        Self {
            bufs,
            second_half: Some(SplitSecondHalf {
                pos: split_pos,
                split_buf_second_half: Some(split_buf_second_half),
            }),
        }
    }

    /// Returns the first half of the split.
    pub fn first_half(&mut self) -> &mut [IoVec<&'a [u8]>] {
        if let Some(second_half) = &self.second_half {
            // we need to include the buffer under the split position too
            &mut self.bufs[0..=second_half.pos]
        } else {
            &mut self.bufs[..]
        }
    }

    /// Returns the second half of the split, reconstructing the split buffer in
    /// the middle, if necessary, consuming the split in the process.
    pub fn into_second_half(self) -> &'a mut [IoVec<&'a [u8]>] {
        if let Some(second_half) = self.second_half {
            // If the buffer at the boundary was split, we need to restore it
            // first. Otherwise the buffers were split at a buffer boundary so
            // we can just return the second half of the split.
            if let Some(split_buf_second_half) =
                second_half.split_buf_second_half
            {
                // See notes in `Self::split_within_buffer`: the pointers here
                // refer to the same buffer at `bufs[split_pos]`, so all we're
                // doing is resizing the slice at that position to be the second
                // half of the original slice, that was untouched since creating
                // this split.
                let split_buf_second_half = unsafe {
                    let slice = std::slice::from_raw_parts(
                        split_buf_second_half.ptr,
                        split_buf_second_half.len,
                    );
                    IoVec::from_slice(slice)
                };

                // restore the second half of the split buffer
                self.bufs[second_half.pos] = split_buf_second_half;
            }
            // return a slice to the buffers starting at the split position
            &mut self.bufs[second_half.pos..]
        } else {
            // otherwise there is no second half, so we return an empty slice
            let write_buf_len = self.bufs.len();
            &mut self.bufs[write_buf_len..]
        }
    }
}

/// Information about a torrent's storage details, such as the piece count and
/// length, download length, etc.
#[derive(Clone, Debug)]
pub(crate) struct StorageInfo {
    /// The number of pieces in the torrent.
    pub piece_count: usize,
    /// The nominal length of a piece.
    pub piece_len: u32,
    /// The length of the last piece in torrent, which may differ from the
    /// normal piece length if the download size is not an exact multiple of the
    /// piece length.
    pub last_piece_len: u32,
    /// The sum of the length of all files in the torrent.
    pub download_len: u64,
    /// The download destination of the torrent.
    ///
    /// In case of a single torrent file, this is the path of the file. In case
    /// of a multi-file torrent, this is the path of the directory containing
    /// those files.
    pub download_path: PathBuf,
    /// The paths and lengths of the torrent files.
    pub structure: FsStructure,
}

impl StorageInfo {
    /// Extracts storage related information from the torrent metainfo.
    pub fn new(metainfo: &Metainfo, download_dir: &Path) -> Self {
        let piece_count = metainfo.piece_count();
        let download_len = metainfo.structure.download_len();
        let piece_len = metainfo.piece_len;
        let last_piece_len =
            download_len - piece_len as u64 * (piece_count - 1) as u64;
        let last_piece_len = last_piece_len as u32;
        // the download path for now is the download directory path joined with
        // the torrent's name as defined in the metainfo
        let download_path = download_dir.join(&metainfo.name);

        Self {
            piece_count,
            piece_len,
            last_piece_len,
            download_len,
            download_path,
            structure: metainfo.structure.clone(),
        }
    }

    /// Returns the zero-based indices of the files of torrent that intersect
    /// with the piece.
    pub fn files_intersecting_piece(
        &self,
        index: PieceIndex,
    ) -> Result<Range<FileIndex>> {
        log::trace!("Returning files intersecting piece {}", index);
        let piece_offset = index as u64 * self.piece_len as u64;
        let piece_end = piece_offset + self.piece_len(index)? as u64;
        let files = self
            .structure
            .files_intersecting_bytes(piece_offset..piece_end);
        Ok(files)
    }

    /// Returns the length of the piece at the given index.
    // TODO: consider panicking instead, as it's more or less an application
    // error to provide an out of bounds index
    pub fn piece_len(&self, index: PieceIndex) -> Result<u32> {
        if index == self.piece_count - 1 {
            Ok(self.last_piece_len)
        } else if index < self.piece_count - 1 {
            Ok(self.piece_len)
        } else {
            log::error!("Piece {} is invalid for torrent: {:?}", index, self);
            Err(Error::InvalidPieceIndex)
        }
    }
}

/// Defines the file system structure of the download.
#[derive(Clone, Debug)]
pub enum FsStructure {
    /// This is a single file download.
    // TODO: consider changing this back to just File { len: u64 } to not have
    // to copy the path
    File(FileInfo),
    /// The download is for multiple files, possibly with nested directories.
    Archive {
        /// When all files in the torrent are viewed as a single contiguous byte
        /// array, we can get the offset of a file in torrent. The file's last
        /// byte offset in torrent is the key of this map, for helping us with
        /// lookups of which piece bytes are contained in file.
        files: Vec<FileInfo>,
    },
}

impl FsStructure {
    /// Returns the total download size in bytes.
    ///
    /// Note that this is an O(n) operation for archive downloads, where n is
    /// the number of files, so this value should ideally be cached.
    pub fn download_len(&self) -> u64 {
        match self {
            Self::File(file) => file.len,
            Self::Archive { files } => files.iter().map(|f| f.len).sum(),
        }
    }

    /// Returns the files that overlap with the given left-inclusive range of
    /// bytes, where `bytes.start` is the offset and `bytes.end` is one past the
    /// last byte offset.
    pub fn files_intersecting_bytes(
        &self,
        byte_range: Range<u64>,
    ) -> Range<FileIndex> {
        match self {
            // when torrent only has one file, only that file can be returned
            //
            // TODO: consider whether to return an error, an empty range, panic,
            // or do a noop if the range is invalid (outside the first or last
            // byte offsets of our file)
            Self::File(_) => 0..1,
            Self::Archive { files } => {
                // find the index of the first file that contains the first byte
                // of the given range
                let first_matching_index = match files
                    .iter()
                    .enumerate()
                    .find(|(_, file)| {
                        // check if the file's byte range contains the first
                        // byte of the given range
                        file.byte_range().contains(&byte_range.start)
                    })
                    .map(|(index, _)| index)
                {
                    Some(index) => index,
                    None => return 0..0,
                };

                // the resulting files
                let mut file_range =
                    first_matching_index..first_matching_index + 1;

                // Find the the last file that contains the last byte of the
                // given range, starting at the file after the above found one.
                //
                // NOTE: the order of `enumerate` and `skip` matters as
                // otherwise we'd be getting relative indices
                for (index, file) in
                    files.iter().enumerate().skip(first_matching_index + 1)
                {
                    // stop if file's first byte is not contained by the given
                    // byte range (is at or past the end of the byte range we're
                    // looking for)
                    if !byte_range.contains(&file.torrent_offset) {
                        break;
                    }

                    // note that we need to add one to the end as this is
                    // a left-inclusive range, so we want the end (excluded) to
                    // be one past the actually included value
                    file_range.end = index + 1;
                }

                file_range
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_at_mut_inclusive() {
        let mut buf = vec![0, 1, 2, 3, 4];
        let slice = buf.as_mut_slice();

        let (a, b) = split_at_mut_inclusive(slice, 0);
        assert_eq!(a, &[0]);
        assert_eq!(b, &[1, 2, 3, 4]);

        let (a, b) = split_at_mut_inclusive(slice, 1);
        assert_eq!(a, &[0, 1]);
        assert_eq!(b, &[2, 3, 4]);

        let (a, b) = split_at_mut_inclusive(slice, 2);
        assert_eq!(a, &[0, 1, 2]);
        assert_eq!(b, &[3, 4]);

        let (a, b) = split_at_mut_inclusive(slice, 3);
        assert_eq!(a, &[0, 1, 2, 3]);
        assert_eq!(b, &[4]);

        let (a, b) = split_at_mut_inclusive(slice, 4);
        assert_eq!(a, &[0, 1, 2, 3, 4]);
        assert_eq!(b, &[]);
    }

    // Tests that splitting of the blocks that align with the file boundary at the
    // last block is a noop.
    //
    // -----------------------------------
    // | file slice: 32                  |
    // -----------------------------------
    // | block: 16      | block: 16      |
    // -----------------------------------
    #[test]
    fn test_split_bufs_same_size_as_file() {
        let slice = FileSlice { offset: 0, len: 32 };
        let blocks =
            vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];
        let blocks_len: usize = blocks.iter().map(Vec::len).sum();

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut split = slice.split_bufs_at_boundary(&mut bufs);

        // we should have both buffers
        assert_eq!(split.first_half().len(), 2);
        // there was no split
        assert!(split.second_half.is_none());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = split
            .first_half()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half has the same bytes as the blocks
        let expected_first_half: Vec<_> = blocks.iter().flatten().collect();
        assert_eq!(first_half.len(), slice.len as usize);
        assert_eq!(first_half.len(), blocks_len);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer, which should be empty
        let second_half = split.into_second_half();
        assert!(second_half.is_empty());
    }

    // Tests that splitting of the blocks whose combined length is smaller than
    // that of the file is a noop.
    //
    // --------------------------------------------
    // | file slice: 42                           |
    // --------------------------------------------
    // | block: 16      | block: 16      |
    // -----------------------------------
    #[test]
    fn test_split_bufs_smaller_than_file() {
        let slice = FileSlice { offset: 0, len: 42 };
        let blocks =
            vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];
        let blocks_len: usize = blocks.iter().map(Vec::len).sum();

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut split = slice.split_bufs_at_boundary(&mut bufs);

        // we should have both buffers
        assert_eq!(split.first_half().len(), 2);
        // there was no split
        assert!(split.second_half.is_none());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = split
            .first_half()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half has the same bytes as the blocks
        let expected_first_half: Vec<_> = blocks.iter().flatten().collect();
        assert_eq!(first_half.len(), blocks_len);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer, which should be empty
        let second_half = split.into_second_half();
        assert!(second_half.is_empty());
    }

    // Tests splitting of the blocks that do not align with file boundary at the
    // last block.
    //
    // ------------------------------
    // | file slice: 25             |
    // -----------------------------------
    // | block: 16      | block: 16 ^    |
    // -----------------------------^-----
    //                              ^
    //              split here into 9 and 7 long halves
    #[test]
    fn test_split_bufs_larger_than_file_at_last_block() {
        let slice = FileSlice { offset: 0, len: 25 };
        let blocks =
            vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut split = slice.split_bufs_at_boundary(&mut bufs);

        // we should have both buffers
        assert_eq!(split.first_half().len(), 2);

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = split
            .first_half()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half is just the file slice number of bytes
        let expected_first_half: Vec<_> =
            blocks.iter().flatten().take(slice.len as usize).collect();
        assert_eq!(first_half.len(), slice.len as usize);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer
        let second_half = split.into_second_half();
        // compare the contents of the second half of the split: convert it
        // to a flat vector for easier comparison
        let second_half: Vec<_> =
            second_half.iter().map(IoVec::as_slice).flatten().collect();
        assert_eq!(second_half.len(), 7);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(slice.len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }

    // Tests splitting of the blocks that do not align with file boundary.
    //
    // ------------------------------
    // | file slice: 25             |
    // ----------------------------------------------------
    // | block: 16      | block: 16 ^    | block: 16      |
    // -----------------------------^----------------------
    //                              ^
    //              split here into 9 and 7 long halves
    #[test]
    fn test_split_bufs_larger_than_file() {
        let slice = FileSlice { offset: 0, len: 25 };
        let blocks = vec![
            (0..16).collect::<Vec<u8>>(),
            (16..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
        ];

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut split = slice.split_bufs_at_boundary(&mut bufs);

        // we should have only the first two buffers
        assert_eq!(split.first_half().len(), 2);
        assert!(split.second_half.is_some());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = split
            .first_half()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half is just the file slice number of bytes
        let expected_first_half: Vec<_> =
            blocks.iter().flatten().take(slice.len as usize).collect();
        assert_eq!(first_half.len(), slice.len as usize);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer
        let second_half = split.into_second_half();
        // compare the contents of the second half of the split: convert it to
        // a flat vector for easier comparison
        let second_half: Vec<_> =
            second_half.iter().map(IoVec::as_slice).flatten().collect();
        // the length should be the length of the second half the split buffer
        // as well as the remaining block's length
        assert_eq!(second_half.len(), 7 + 16);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(slice.len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }

    #[test]
    fn test_files_intersecting_pieces() {
        // single file
        let piece_count = 4;
        let piece_len = 4;
        let last_piece_len = 2;
        // 3 full length pieces; 1 smaller piece,
        let download_len = 3 * 4 + 2;
        let structure = FsStructure::File(FileInfo {
            path: PathBuf::from("/bogus"),
            torrent_offset: 0,
            len: download_len,
        });
        let info = StorageInfo {
            piece_count,
            piece_len,
            last_piece_len,
            download_len,
            download_path: PathBuf::from("/"),
            structure,
        };
        // all 4 pieces are in the same file
        assert_eq!(info.files_intersecting_piece(0).unwrap(), 0..1);
        assert_eq!(info.files_intersecting_piece(1).unwrap(), 0..1);
        assert_eq!(info.files_intersecting_piece(2).unwrap(), 0..1);
        assert_eq!(info.files_intersecting_piece(3).unwrap(), 0..1);

        // multi-file
        //
        // pieces: (index:first byte offset)
        // --------------------------------------------------------------------
        // |0:0         |1:16          |2:32          |3:48          |4:64    |
        // --------------------------------------------------------------------
        // files: (index:first byte offset,last byte offset)
        // --------------------------------------------------------------------
        // |0:0,8 |1:9,19  |2:20,26|3:27,35 |4:36,47  |5:48,63       |6:64,71 |
        // --------------------------------------------------------------------
        let files = vec![
            FileInfo {
                path: PathBuf::from("/0"),
                torrent_offset: 0,
                len: 9,
            },
            FileInfo {
                path: PathBuf::from("/1"),
                torrent_offset: 9,
                len: 11,
            },
            FileInfo {
                path: PathBuf::from("/2"),
                torrent_offset: 20,
                len: 7,
            },
            FileInfo {
                path: PathBuf::from("/3"),
                torrent_offset: 27,
                len: 9,
            },
            FileInfo {
                path: PathBuf::from("/4"),
                torrent_offset: 36,
                len: 12,
            },
            FileInfo {
                path: PathBuf::from("/5"),
                torrent_offset: 48,
                len: 16,
            },
            FileInfo {
                path: PathBuf::from("/6"),
                torrent_offset: 64,
                len: 8,
            },
        ];
        let download_len: u64 = files.iter().map(|f| f.len).sum();
        // sanity check that the offsets in the files above correctly follow
        // each other and that they add up to the total download length
        debug_assert_eq!(
            files.iter().fold(0, |offset, file| {
                debug_assert_eq!(offset, file.torrent_offset);
                offset + file.len
            }),
            download_len,
        );
        let piece_count: usize = 5;
        let piece_len: u32 = 16;
        let last_piece_len: u32 = 8;
        // sanity check that full piece lengths and last piece length equals the
        // total download length
        debug_assert_eq!(
            (piece_count as u64 - 1) * piece_len as u64 + last_piece_len as u64,
            download_len
        );
        let info = StorageInfo {
            piece_count,
            piece_len,
            last_piece_len,
            download_len,
            download_path: PathBuf::from("/"),
            structure: FsStructure::Archive { files },
        };
        // piece 0 intersects with files 0 and 1
        assert_eq!(info.files_intersecting_piece(0).unwrap(), 0..2);
        // piece 1 intersects with files 1, 2, 3
        assert_eq!(info.files_intersecting_piece(1).unwrap(), 1..4);
        // piece 2 intersects with files 3 and 4
        assert_eq!(info.files_intersecting_piece(2).unwrap(), 3..5);
        // piece 3 intersects with only file 5
        assert_eq!(info.files_intersecting_piece(3).unwrap(), 5..6);
        // last piece 4 intersects with only file 6
        assert_eq!(info.files_intersecting_piece(4).unwrap(), 6..7);
        // piece 5 is invalid
        assert!(info.files_intersecting_piece(5).is_err());
    }

    #[test]
    fn test_files_intersecting_bytes() {
        // single file
        let structure = FsStructure::File(FileInfo {
            path: PathBuf::from("/bogus"),
            torrent_offset: 0,
            len: 12341234,
        });
        assert_eq!(structure.files_intersecting_bytes(0..0), 0..1);
        assert_eq!(structure.files_intersecting_bytes(0..1), 0..1);
        assert_eq!(structure.files_intersecting_bytes(0..12341234), 0..1);

        // multi-file
        let structure = FsStructure::Archive {
            files: vec![
                FileInfo {
                    path: PathBuf::from("/bogus0"),
                    torrent_offset: 0,
                    len: 4,
                },
                FileInfo {
                    path: PathBuf::from("/bogus1"),
                    torrent_offset: 4,
                    len: 9,
                },
                FileInfo {
                    path: PathBuf::from("/bogus2"),
                    torrent_offset: 13,
                    len: 3,
                },
                FileInfo {
                    path: PathBuf::from("/bogus3"),
                    torrent_offset: 16,
                    len: 10,
                },
            ],
        };
        // bytes only in the first file
        assert_eq!(structure.files_intersecting_bytes(0..4), 0..1);
        // bytes intersecting two files
        assert_eq!(structure.files_intersecting_bytes(0..5), 0..2);
        // bytes overlapping with two files
        assert_eq!(structure.files_intersecting_bytes(0..13), 0..2);
        // bytes intersecting three files
        assert_eq!(structure.files_intersecting_bytes(0..15), 0..3);
        // bytes intersecting all files
        assert_eq!(structure.files_intersecting_bytes(0..18), 0..4);
        // bytes intersecting the last byte of the last file
        assert_eq!(structure.files_intersecting_bytes(25..26), 3..4);
        // bytes overlapping with two files in the middle
        assert_eq!(structure.files_intersecting_bytes(4..16), 1..3);
        // bytes intersecting only one byte of two files each, among the middle
        // of all files
        assert_eq!(structure.files_intersecting_bytes(8..14), 1..3);
        // bytes intersecting only one byte of one file, among the middle of all
        // files
        assert_eq!(structure.files_intersecting_bytes(13..14), 2..3);
        // bytes not intersecting any files
        assert_eq!(structure.files_intersecting_bytes(30..38), 0..0);
    }
}
