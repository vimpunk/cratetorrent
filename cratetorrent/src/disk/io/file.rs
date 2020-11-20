use std::{
    fs::{File, OpenOptions},
    os::unix::io::AsRawFd,
    path::Path,
};

use nix::sys::uio::pwritev;

use crate::{
    disk::error::*,
    iovecs::{IoVec, IoVecs},
    storage_info::FileSlice,
    FileInfo,
};

pub(super) struct TorrentFile {
    pub info: FileInfo,
    pub handle: File,
}

impl TorrentFile {
    /// Opens the file in create, read, and write modes at the path of combining the
    /// download directory and the path defined in the file info.
    pub fn new(
        download_dir: &Path,
        info: FileInfo,
    ) -> Result<Self, NewTorrentError> {
        log::trace!(
            "Opening and creating file {:?} in dir {:?}",
            info,
            download_dir
        );
        let path = download_dir.join(&info.path);
        let handle = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&path)
            .map_err(|e| {
                log::warn!("Failed to open file {:?}", path);
                NewTorrentError::Io(e)
            })?;
        debug_assert!(path.exists());
        Ok(Self { info, handle })
    }

    /// Writes to file at most the slice length number of bytes of blocks at the
    /// file slice's offset, using pwritev, called repeteadly until all blocks are
    /// written to disk.
    ///
    /// It returns the slice of blocks that weren't written to disk. That is, it
    /// returns the second half of `blocks` as though they were split at the
    /// `file_slice.len` offset. If all blocks were written to disk an empty
    /// slice is returned.
    ///
    /// # Important
    ///
    /// Since the syscall may be invoked repeatedly to perform disk IO, this
    /// means that this operation is not guaranteed to be atomic.
    pub fn write<'a>(
        &self,
        file_slice: FileSlice,
        blocks: &'a mut [IoVec<&'a [u8]>],
    ) -> Result<&'a mut [IoVec<&'a [u8]>], WriteError> {
        let mut iovecs = IoVecs::bounded(blocks, file_slice.len as usize);
        // the write buffer cannot be larger than the file slice we want to
        // write to
        debug_assert!(
            iovecs
                .as_slice()
                .iter()
                .map(|iov| iov.as_slice().len() as u64)
                .sum::<u64>()
                <= file_slice.len
        );

        // IO syscalls are not guaranteed to transfer the whole input buffer in one
        // go, so we need to repeat until all bytes have been confirmed to be
        // transferred to disk (or an error occurs)
        let mut total_write_count = 0;
        while !iovecs.as_slice().is_empty() {
            let write_count = pwritev(
                self.handle.as_raw_fd(),
                iovecs.as_slice(),
                file_slice.offset as i64,
            )
            .map_err(|e| {
                log::warn!("File {:?} write error: {}", self.info.path, e);
                // FIXME: convert actual error here
                WriteError::Io(std::io::Error::last_os_error())
            })?;

            // tally up the total write count
            total_write_count += write_count;

            // no need to advance write buffers cursor if we've written
            // all of it to file--in that case, we can just split the iovecs
            // and return the second half, consuming the first half
            if total_write_count as u64 == file_slice.len {
                break;
            }

            // advance the buffer cursor in iovecs by the number of bytes
            // transferred
            iovecs.advance(write_count);
        }

        Ok(iovecs.into_tail())
    }
}
