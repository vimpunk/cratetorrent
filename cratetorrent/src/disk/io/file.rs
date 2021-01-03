use std::{
    fs::{File, OpenOptions},
    path::Path,
    io::ErrorKind
};

use crate::{
    disk::error::*,
    storage_info::FileSlice,
    FileInfo,
};

use cfg_if::cfg_if;

#[cfg(target_os = "linux")]
use crate::iovecs::{self, IoVec, IoVecs};


// a convenience macro for handling ErrorKind::Interrupted,
// zero-byte reads and the like
macro_rules! unwrap_read_res {
    ($res:expr) => {
        match $res {
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Err(ReadError::MissingData),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),

            // If there was nothing to read from file, it means we tried to
            // read a piece from a portion of a file not yet downloaded or
            // otherwise missing
            Ok(0) => return Err(ReadError::MissingData),

            Ok(written) => written,
        }
    }
}

// a convenience macro for handling ErrorKind::Interrupted,
// zero-byte writes and the like
macro_rules! unwrap_write_res {
    ($res:expr) => {
        match $res {
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),

            // Write syscalls never return 0 these days (unless buf.len == 0).
            // If that happened, something went terribly wrong;
            // io::ErrorKind even has a dedicated variant for it!
            // Anyway, we don't expect them to happen.
            Ok(0) => {
                return Err(std::io::Error::new(ErrorKind::WriteZero, "failed to write whole buffer").into());
            }

            Ok(written) => written,
        }
    }
}


pub(crate) struct TorrentFile {
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
    /// file slice's offset, using pwritev, called repeatedly until all blocks are
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
    #[cfg(target_os = "linux")]
    pub fn write_vectored<'a>(
        &self,
        file_slice: FileSlice,
        blocks: &'a mut [IoVec<&'a [u8]>],
    ) -> Result<&'a mut [IoVec<&'a [u8]>], WriteError> {
        use std::os::unix::io::AsRawFd;
        use nix::sys::uio::{pwritev};

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
            let write_res = pwritev(
                self.handle.as_raw_fd(),
                iovecs.as_slice(),
                file_slice.offset as i64,
            )
            .map_err(|sys_err| {
                let err = convert_nix_error(sys_err);
                log::warn!("File {:?} write error: {}", self.info.path, err);
                err
            });

            let write_count = unwrap_write_res!(write_res);

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

    pub fn write<'a>(&self, file_slice: FileSlice, blocks: &'a [u8]) -> Result<&'a [u8], WriteError> {
        let len = file_slice.len as usize;
        let content = &blocks[..len];

        write_all_at(&self.handle, content, file_slice.offset)
            .map_err(|err| {
                log::warn!("File {:?} write error: {}", self.info.path, err);
                err
            })?;

        Ok(&blocks[len..])
    }

    /// Reads from file at most the slice length number of bytes of blocks at
    /// the file slice's offset, using preadv, called repeatedly until all
    /// blocks are read from disk.
    ///
    /// It returns the slice of block buffers that weren't filled by the
    /// disk-read. That is, it returns the second half of `blocks` as though
    /// they were split at the `file_slice.len` offset. If all blocks were read
    /// from disk an empty slice is returned.
    ///
    /// # Important
    ///
    /// Since the syscall may be invoked repeatedly to perform disk IO, this
    /// means that this operation is not guaranteed to be atomic.
    #[cfg(target_os = "linux")]
    pub fn read_vectored<'a>(
        &self,
        file_slice: FileSlice,
        mut iovecs: &'a mut [IoVec<&'a mut [u8]>],
    ) -> Result<&'a mut [IoVec<&'a mut [u8]>], ReadError> {
        use std::os::unix::io::AsRawFd;
        use nix::sys::uio::preadv;

        // This is simpler than the write implementation as the preadv method
        // stops reading in from the file if reaching EOF. We do need to advance
        // the iovecs read buffer cursor after a read as we may want to read
        // from other files after this one, in which case the cursor should
        // be on the next byte to read to.

        // IO syscalls are not guaranteed to transfer the whole input buffer in one
        // go, so we need to repeat until all bytes have been confirmed to be
        // transferred to disk (or an error occurs)
        let mut total_read_count = 0;
        while !iovecs.is_empty() && (total_read_count as u64) < file_slice.len {
            let read_res = preadv(
                self.handle.as_raw_fd(),
                iovecs,
                file_slice.offset as i64,
            )
            .map_err(|sys_err| {
                let err = convert_nix_error(sys_err);
                log::warn!("File {:?} read error: {}", self.info.path, err);
                err
            });

            let read_count = unwrap_read_res!(read_res);

            // tally up the total read count
            total_read_count += read_count;

            // advance the buffer cursor in iovecs by the number of bytes
            // transferred
            iovecs = iovecs::advance(iovecs, read_count);
        }

        Ok(iovecs)
    }

    pub fn read<'a>(&self, file_slice: FileSlice, blocks: &'a mut [u8]) -> Result<&'a mut [u8], ReadError> {
        let len = file_slice.len as usize;

        read_exact_at(&self.handle, &mut blocks[..len], file_slice.offset)
            .map_err(|err| {
                log::warn!("File {:?} read error: {}", self.info.path, err);
                err
            })?;

        Ok(&mut blocks[len..])
    }
}

#[cfg(target_os = "linux")]
fn convert_nix_error(err: nix::Error) -> std::io::Error {
    match err {
        nix::Error::Sys(errno) => std::io::Error::from_raw_os_error(errno as i32),

        // preadv/pwrite never return them, but better safe than sorry
        nix::Error::InvalidPath |
        nix::Error::InvalidUtf8 |
        nix::Error::UnsupportedOperation => {
            log::warn!("Unexpected nix::Error kind ({}), falling back to last_os_error", err);
            std::io::Error::last_os_error()
        },
    }
}

// Here's where the most effective implementation of `write_at/read_at` is chosen.
// The trick is, linux-specific APIs do not modify the "seek cursor", while Windows' and others' do.
// This discrepancy doesn't matter because nothing in this crate relies on this cursor
// (the read/write offset is always provided explicitly).
//
// FIXME: On the other hand, if `TorrentInfo.handle` will ever become exposed in our public API,
// this "jumping cursor" can potentially be observed by the outside world. It shouldn't be
// a problem either because, well, I fail to see why someone would want to interact with
// the file in a way where the cursor's positioning matters. But, at least, this should be
// clearly documented and the user must be warned to perform his own seeks if need for
// unaccounted reads/writes presents itself...
cfg_if! {
    if #[cfg(any(unix, target_os = "redox", target_os = "vxworks", target_os = "hermit"))] {
        use std::os::unix::fs::FileExt;

        #[inline]
        fn write_all_at(file: &File, buf: &[u8], offset: u64) -> Result<(), WriteError> {
            file.write_all_at(buf, offset).map_err(Into::into)
        }

        #[inline]
        fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<(), ReadError> {
            file.read_exact_at(buf, offset)
                .map_err(|e| match e.kind() {
                    ErrorKind::UnexpectedEof => ReadError::MissingData,
                    _ => e.into()
                })

        }
    } else if #[cfg(windows)] {
        use std::os::windows::fs::FileExt;

        fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> Result<(), WriteError> {
            while !buf.is_empty() {
                let written = unwrap_write_res!(file.seek_write(buf, offset));
                offset += written;
                buf = &buf[written..];
            }

            Ok(())
        }

        fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> Result<(), ReadError> {
            while !buf.is_empty() {
                let read = unwrap_read_res!(file.seek_read(buf, offset));
                offset += read;
                buf = &mut buf[read..];
            }

            Ok(())
        }
    } else {
        fn write_all_at(file: &File, buf: &[u8], offset: u64) -> Result<(), WriteError> {
            file.seek(SeekFrom(offset))?;
            file.write_all(buf).map_err(Into::into)
        }

        fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<(), ReadError> {
            file.seek(SeekFrom(offset))?;
            file.read_exact(buf).map_err(|e| match e.kind() {
                ErrorKind::UnexpectedEof => ReadError::MissingData,
                _ => e.into()
            })
        }
    }
}


