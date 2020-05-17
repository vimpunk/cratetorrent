pub use nix::sys::uio::IoVec;

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
pub(crate) struct IoVecs<'a> {
    /// The entire view of the underlying buffers.
    bufs: &'a mut [IoVec<&'a [u8]>],
    /// If set, the buffer is bounded by a given boundary, and is effectively
    /// "split". This includes metadata to reconstruct the second half of the
    /// split.
    split: Option<Split>,
}

impl<'a> IoVecs<'a> {
    /// Bounds the iovecs, potentially spliting it in two, if the total byte
    /// count of the buffers exceeds the limit.
    ///
    /// This is most useful when writing to a slice of a file while ensuring
    /// that the file is not extended if the input buffers are larger than the
    /// length of the file slice.
    ///
    /// If the total size of the buffers exceeds the max length, the buffers are
    /// split such that the first half returned is the portion of the buffers
    /// that can be written to the file, while the second half is the remainder
    /// of the buffer, to be reused later. If the size of the total size of the
    /// buffers is smaller than or equal to the slice, this is essentially
    /// a noop.
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
    /// # Arguments
    ///
    /// * `bufs` - A slice that points to a contiguous list of IO vectors, which
    ///     in turn point to the actual blocks of memory used for file IO.
    /// * `max_len` - The maximum byte count of the total number of bytes in the
    ///     IO vectors.
    ///
    /// # Panics
    ///
    /// The constructor panics if the max length is 0.
    ///
    /// # Important
    ///
    /// In reality, the situation here is more complex than just splitting
    /// buffers in half, but this is taken care of by the [`IoVecs`]
    /// implementation.
    ///
    /// However, the abstraction leaks through because the iovec at which the
    /// buffers were split may be shrunk to such a size as would enable all
    /// buffers to stay within the file slice length. This can be restored using
    /// [`IoVecs::into_remainder`], but until this is called, the original
    /// buffers cannot be used, which is enforced by the borrow checker.
    pub fn bounded(bufs: &'a mut [IoVec<&'a [u8]>], max_len: usize) -> Self {
        if max_len == 0 {
            panic!("IoVecs max length should be larger than 0");
        }

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
            bufs_len += buf.as_slice().len();
            bufs_len >= max_len
        }) {
            Some(pos) => pos,
            None => return IoVecs::unbounded(bufs),
        };

        // If we're here, it means that the total buffers length exceeds the
        // slice length and we must split the buffers.
        if bufs_len == max_len {
            // The buffer boundary aligns with the file boundary. There are two
            // cases here:
            // 1. the buffers are the same length as the file, in which case
            //    there is nothing to split,
            // 2. or we just need to split at the buffer boundary.
            if bufs_split_pos + 1 == bufs.len() {
                // the split position is the end of the last buffer, so there is
                // nothing to split
                IoVecs::unbounded(bufs)
            } else {
                // we can split at the buffer boundary
                IoVecs::split_at_buffer_boundary(bufs, bufs_split_pos)
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
            let buf_offset = bufs_len - buf_to_split.len();
            let buf_split_pos = max_len - buf_offset;
            debug_assert!(buf_split_pos < buf_to_split.len());

            IoVecs::split_within_buffer(bufs, bufs_split_pos, buf_split_pos)
        }
    }

    /// Creates an unbounded `IoVec`, meaning that no split is necessary.
    pub fn unbounded(bufs: &'a mut [IoVec<&'a [u8]>]) -> Self {
        Self { bufs, split: None }
    }

    /// Creates a "clean split", in which the split occurs at the buffer
    /// boundary and `bufs` need only be split at the slice level.
    fn split_at_buffer_boundary(
        bufs: &'a mut [IoVec<&'a [u8]>],
        pos: usize,
    ) -> Self {
        Self {
            bufs,
            split: Some(Split {
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
        // `IoVecs`. This, however, is also safe, as we don't leak the
        // raw buffer and pointers for other code to unsafely reconstruct
        // the slice. The slice is only reconstructined in
        // `IoVecs::into_second_half`, assigning it to the `IoVec` at
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
            split: Some(Split {
                pos: split_pos,
                split_buf_second_half: Some(split_buf_second_half),
            }),
        }
    }

    /// Returns the first half of the split.
    pub fn buffers(&mut self) -> &mut [IoVec<&'a [u8]>] {
        if let Some(second_half) = &self.split {
            // we need to include the buffer under the split position too
            &mut self.bufs[0..=second_half.pos]
        } else {
            &mut self.bufs[..]
        }
    }

    pub fn advance(&mut self, n: usize) {
        todo!();
    }

    /// Returns the second half of the split, reconstructing the split buffer in
    /// the middle, if necessary, consuming the split in the process.
    pub fn into_tail(self) -> &'a mut [IoVec<&'a [u8]>] {
        if let Some(second_half) = self.split {
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

#[derive(Debug)]
struct Split {
    /// The position at which to split `IoVecs::bufs`. Note that this is
    /// inclusive, i.e. we split at `[0, pos]`.
    pos: usize,
    /// If set, it means that the buffer at `bufs[split_pos]` was further split
    /// in two. It contains the second half of the split buffer.
    split_buf_second_half: Option<RawBuf>,
}

/// A byte slice deconstructed into its raw parts.
#[derive(Debug)]
struct RawBuf {
    ptr: *const u8,
    len: usize,
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

    // Tests that splitting of the blocks that align with the file boundary at
    // the last block is a noop.
    //
    // -----------------------------------
    // | file slice: 32                  |
    // -----------------------------------
    // | block: 16      | block: 16      |
    // -----------------------------------
    #[test]
    fn test_split_bufs_same_size_as_file() {
        let file_len = 32;
        let blocks =
            vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];
        let blocks_len: usize = blocks.iter().map(Vec::len).sum();

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut iovecs = IoVecs::bounded(&mut bufs, 32);

        // we should have both buffers
        assert_eq!(iovecs.buffers().len(), 2);
        // there was no split
        assert!(iovecs.split.is_none());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs
            .buffers()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half has the same bytes as the blocks
        let expected_first_half: Vec<_> = blocks.iter().flatten().collect();
        assert_eq!(first_half.len(), file_len as usize);
        assert_eq!(first_half.len(), blocks_len);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer, which should be empty
        let second_half = iovecs.into_tail();
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
        let file_len = 42;
        let blocks =
            vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];
        let blocks_len: usize = blocks.iter().map(Vec::len).sum();

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut iovecs = IoVecs::bounded(&mut bufs, 42);

        // we should have both buffers
        assert_eq!(iovecs.buffers().len(), 2);
        // there was no split
        assert!(iovecs.split.is_none());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs
            .buffers()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half has the same bytes as the blocks
        let expected_first_half: Vec<_> = blocks.iter().flatten().collect();
        assert_eq!(first_half.len(), blocks_len);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer, which should be empty
        let second_half = iovecs.into_tail();
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
        let file_len = 25;
        let blocks =
            vec![(0..16).collect::<Vec<u8>>(), (16..32).collect::<Vec<u8>>()];

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut iovecs = IoVecs::bounded(&mut bufs, file_len);

        // we should have both buffers
        assert_eq!(iovecs.buffers().len(), 2);

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs
            .buffers()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half is just the file slice number of bytes
        let expected_first_half: Vec<_> =
            blocks.iter().flatten().take(file_len as usize).collect();
        assert_eq!(first_half.len(), file_len as usize);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer
        let second_half = iovecs.into_tail();
        // compare the contents of the second half of the split: convert it
        // to a flat vector for easier comparison
        let second_half: Vec<_> =
            second_half.iter().map(IoVec::as_slice).flatten().collect();
        assert_eq!(second_half.len(), 7);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(file_len as usize).collect();
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
        let file_len = 25;
        let blocks = vec![
            (0..16).collect::<Vec<u8>>(),
            (16..32).collect::<Vec<u8>>(),
            (32..48).collect::<Vec<u8>>(),
        ];

        let mut bufs: Vec<_> =
            blocks.iter().map(|buf| IoVec::from_slice(&buf)).collect();

        let mut iovecs = IoVecs::bounded(&mut bufs, 25);

        // we should have only the first two buffers
        assert_eq!(iovecs.buffers().len(), 2);
        assert!(iovecs.split.is_some());

        // compare the contents of the first half of the split: convert it
        // to a flat vector for easier comparison
        let first_half: Vec<_> = iovecs
            .buffers()
            .iter()
            .map(IoVec::as_slice)
            .flatten()
            .collect();
        // the expected first half is just the file slice number of bytes
        let expected_first_half: Vec<_> =
            blocks.iter().flatten().take(file_len as usize).collect();
        assert_eq!(first_half.len(), file_len as usize);
        assert_eq!(first_half, expected_first_half);

        // restore the second half of the split buffer
        let second_half = iovecs.into_tail();
        // compare the contents of the second half of the split: convert it to
        // a flat vector for easier comparison
        let second_half: Vec<_> =
            second_half.iter().map(IoVec::as_slice).flatten().collect();
        // the length should be the length of the second half the split buffer
        // as well as the remaining block's length
        assert_eq!(second_half.len(), 7 + 16);
        // the expected second half is just the bytes after the file slice number of bytes
        let expected_second_half: Vec<_> =
            blocks.iter().flatten().skip(file_len as usize).collect();
        assert_eq!(second_half, expected_second_half);
    }
}
