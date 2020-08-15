use {
    nix::sys::uio::pwritev,
    sha1::{Digest, Sha1},
    std::{
        collections::{BTreeMap, HashMap},
        fs::{self, File, OpenOptions},
        ops::Range,
        os::unix::io::AsRawFd,
        path::Path,
        sync::{Arc, Mutex},
    },
    tokio::{
        sync::{mpsc, RwLock},
        task,
    },
};

use {
    super::{
        error::*, Alert, AlertReceiver, AlertSender, BatchWrite, Command,
        CommandReceiver, CommandSender, TorrentAlert, TorrentAlertReceiver,
        TorrentAlertSender, TorrentAllocation,
    },
    crate::{
        block_count,
        error::Error,
        iovecs::{IoVec, IoVecs},
        storage_info::{FileSlice, FsStructure, StorageInfo},
        BlockInfo, FileIndex, FileInfo, PieceIndex, Sha1Hash, TorrentId,
    },
};

/// The entity responsible for saving downloaded file blocks to disk and
/// verifying whether downloaded pieces are valid.
pub(super) struct Disk {
    /// Each torrent in engine has a corresponding entry in this hashmap, which
    /// includes various metadata about torrent and the torrent specific alert
    /// channel.
    torrents: HashMap<TorrentId, RwLock<Torrent>>,
    /// Port on which disk IO commands are received.
    cmd_port: CommandReceiver,
    /// Channel on which `Disk` sends alerts to the torrent engine.
    alert_chan: AlertSender,
}

impl Disk {
    /// Creates a new `Disk` instance and returns a command sender and an alert
    /// receiver.
    pub(super) fn new() -> Result<(Self, CommandSender, AlertReceiver)> {
        let (alert_chan, alert_port) = mpsc::unbounded_channel();
        let (cmd_chan, cmd_port) = mpsc::unbounded_channel();
        Ok((
            Self {
                torrents: HashMap::new(),
                cmd_port,
                alert_chan,
            },
            cmd_chan,
            alert_port,
        ))
    }

    /// Starts the disk event loop which is run until shutdown or an
    /// unrecoverable error occurs (e.g. mpsc channel failure).
    pub(super) async fn start(&mut self) -> Result<()> {
        log::info!("Starting disk IO event loop");
        while let Some(cmd) = self.cmd_port.recv().await {
            log::debug!("Disk received command");
            match cmd {
                Command::NewTorrent {
                    id,
                    info,
                    piece_hashes,
                } => {
                    if self.torrents.contains_key(&id) {
                        log::warn!("Torrent {} already allocated", id);
                        self.alert_chan.send(Alert::TorrentAllocation(Err(
                            NewTorrentError::AlreadyExists,
                        )))?;
                        continue;
                    }

                    // NOTE: Do _NOT_ return on failure, we don't want to kill
                    // the disk task due to potential disk IO errors: we just
                    // want to log it and notify engine of it.
                    let torrent_res = Torrent::new(info, piece_hashes);
                    match torrent_res {
                        Ok((torrent, alert_port)) => {
                            log::info!("Torrent {} successfully allocated", id);
                            self.torrents.insert(id, RwLock::new(torrent));
                            // send notificaiton of allocation success
                            self.alert_chan.send(Alert::TorrentAllocation(
                                Ok(TorrentAllocation { id, alert_port }),
                            ))?;
                        }
                        Err(e) => {
                            log::warn!(
                                "Torrent {} allocation failure: {}",
                                id,
                                e
                            );
                            // send notificaiton of allocation failure
                            self.alert_chan
                                .send(Alert::TorrentAllocation(Err(e)))?;
                        }
                    }
                }
                Command::WriteBlock { id, info, data } => {
                    self.write_block(id, info, data).await?;
                }
                Command::Shutdown => {
                    log::info!("Shutting down disk event loop");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Queues a block for writing and fails if the torrent id is invalid.
    ///
    /// If the block could not be written due to IO failure, the torrent is
    /// notified of it.
    async fn write_block(
        &self,
        id: TorrentId,
        info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        log::trace!("Saving torrent {} block {:?} to disk", id, info);

        // check torrent id
        //
        // TODO: maybe we don't want to crash the disk task due to an invalid
        // torrent id: could it be that disk requests for a torrent arrive after
        // a torrent has been removed?
        let torrent = self.torrents.get(&id).ok_or_else(|| {
            log::warn!("Torrent {} not found", id);
            Error::InvalidTorrentId
        })?;
        torrent.write().await.write_block(info, data).await
    }
}

/// Torrent information related to disk IO.
///
/// Contains the in-progress pieces (i.e. the write buffer), metadata about
/// torrent's download and piece sizes, etc.
struct Torrent {
    /// All information concerning this torrent's storage.
    info: StorageInfo,
    /// The channel used to alert a torrent that a block has been written to
    /// disk and/or a piece was completed.
    alert_chan: TorrentAlertSender,
    /// The in-progress piece downloads and disk writes. This is the torrent's
    /// disk write buffer. Each piece is mapped to its index for faster lookups.
    // TODO(https://github.com/mandreyel/cratetorrent/issues/22): Currently
    // there is no upper bound on the in-memory write buffer, so this may lead
    // to OOM.
    pieces: HashMap<PieceIndex, Piece>,
    /// Handles of all files in torrent, opened in advance during torrent
    /// creation.
    ///
    /// Each writer thread will get exclusive access to the file handle it
    /// needs, referring to it directly in the vector (hence the arc).
    ///
    /// Later we will need to make file access more granular, as multiple
    /// concurrent writes to the same file that don't overlap are safe to do.
    // TODO: Is there a way to avoid copying `FileInfo`s here from
    // `self.info.structure`? We could just pass the file info on demand, but
    // that woudl require reallocating this vector every time (to pass a new
    // vector of pairs of `TorrentFile` and `FileInfo`).
    files: Arc<Vec<Mutex<TorrentFile>>>,
    /// The concatenation of all expected piece hashes.
    piece_hashes: Vec<u8>,
    /// Disk IO statistics.
    stats: Stats,
}

impl Torrent {
    /// Creates the file system structure of the torrent and opens the file
    /// handles.
    ///
    /// For a single file, there is a path validity check and then the file is
    /// opened. For multi-file torrents, if there are any subdirectories in the
    /// torrent archive, they are created and all files are opened.
    fn new(
        info: StorageInfo,
        piece_hashes: Vec<u8>,
    ) -> Result<(Self, TorrentAlertReceiver), NewTorrentError> {
        // TODO: since this is done as part of a tokio::task, should we use
        // tokio_fs here?
        if !info.download_dir.is_dir() {
            log::warn!(
                "Creating missing download directory {:?}",
                info.download_dir
            );
            fs::create_dir_all(&info.download_dir)?;
            log::info!("Download directory {:?} created", info.download_dir);
        }

        let files = match &info.structure {
            FsStructure::File(file) => {
                log::debug!(
                    "Torrent is single {} bytes long file {:?}",
                    file.len,
                    file.path
                );
                vec![Mutex::new(TorrentFile::new(
                    &info.download_dir,
                    file.clone(),
                )?)]
            }
            FsStructure::Archive { files } => {
                debug_assert!(!files.is_empty());
                log::debug!("Torrent is multi file: {:?}", files);
                log::debug!("Setting up directory structure");

                let mut torrent_files = Vec::with_capacity(files.len());
                for file in files.iter() {
                    let path = info.download_dir.join(&file.path);
                    // file or subdirectory in download root must not exist if
                    // download root does not exists
                    debug_assert!(!path.exists());
                    debug_assert!(path.is_absolute());

                    // get the parent of the file path: if there is one (i.e.
                    // this is not a file in the torrent root), and doesn't
                    // exist, create it
                    if let Some(subdir) = path.parent() {
                        if !subdir.exists() {
                            log::info!("Creating torrent subdir {:?}", subdir);
                            fs::create_dir_all(&subdir).map_err(|e| {
                                log::warn!(
                                    "Failed to create subdir {:?}",
                                    subdir
                                );
                                NewTorrentError::Io(e)
                            })?;
                        }
                    }

                    // open the file and get a handle to it
                    //
                    // TODO: is there a clean way of avoiding creating the path
                    // buffer twice?
                    torrent_files.push(Mutex::new(TorrentFile::new(
                        &info.download_dir,
                        file.clone(),
                    )?));
                }
                torrent_files
            }
        };

        let (alert_chan, alert_port) = mpsc::unbounded_channel();

        Ok((
            Self {
                info,
                alert_chan,
                pieces: HashMap::new(),
                files: Arc::new(files),
                piece_hashes,
                stats: Stats::default(),
            },
            alert_port,
        ))
    }

    async fn write_block(
        &mut self,
        info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        log::trace!("Saving block {:?} to disk", info);

        let piece_index = info.piece_index;
        if !self.pieces.contains_key(&piece_index) {
            if let Err(e) = self.start_new_piece(info) {
                self.alert_chan.send(TorrentAlert::BatchWrite(Err(e)))?;
                // return with ok as the disk task itself shouldn't be aborted
                // due to invalid input
                return Ok(());
            }
        }
        let piece = self
            .pieces
            .get_mut(&piece_index)
            .expect("Newly inserted piece not present");

        piece.enqueue_block(info.offset, data);

        // if the piece has all its blocks, it means we can hash it and save it
        // to disk and clear its write buffer
        if piece.is_complete() {
            // TODO: remove from in memory store only if the disk write
            // succeeded (otherwise we need to retry later)
            let piece = self.pieces.remove(&piece_index).unwrap();
            let piece_len = self.info.piece_len;
            let files = Arc::clone(&self.files);

            log::info!(
                "Piece {} is complete ({} bytes), flushing {} block(s) to disk",
                info.piece_index,
                piece_len,
                piece.blocks.len()
            );

            // don't block the reactor with the potentially expensive hashing
            // and sync file writing
            let write_result = task::spawn_blocking(move || {
                let is_piece_valid = piece.matches_hash();

                // save piece to disk if it's valid
                let blocks = if is_piece_valid {
                    log::info!("Piece {} is valid", piece_index);
                    let piece_torrent_offset = piece_index as u64 * piece_len as u64;
                    piece.write(piece_torrent_offset, &*files)?;

                    // collect block infos for torrent to identify which
                    // blocks were written to disk
                    let blocks = piece
                        .blocks
                        .iter()
                        .map(|(offset, block)| BlockInfo {
                            piece_index: info.piece_index,
                            offset: *offset,
                            len: block.len() as u32,
                        })
                        .collect();

                    blocks
                } else {
                    log::warn!("Piece {} is NOT valid", info.piece_index);
                    Vec::new()
                };

                Ok((is_piece_valid, blocks))
            })
            .await
            // our code doesn't panic in the task so until better strategies
            // are devised, unwrap here
            .expect("disk IO write task panicked");

            // We don't error out on disk write failure as we don't want to
            // kill the disk task due to potential disk IO errors (which may
            // happen from time to time). We alert torrent of this failure and
            // return normally.
            //
            // TODO(https://github.com/mandreyel/cratetorrent/issues/23): also
            // place back piece write buffer in torrent and retry later
            match write_result {
                Ok((is_piece_valid, blocks)) => {
                    // record write statistics if the piece is valid
                    if is_piece_valid {
                        self.stats.write_count += piece_len as u64;
                    }

                    // alert torrent of block writes and piece completion
                    self.alert_chan.send(TorrentAlert::BatchWrite(Ok(
                        BatchWrite {
                            blocks,
                            is_piece_valid: Some(is_piece_valid),
                        },
                    )))?;
                }
                Err(e) => {
                    log::warn!("Disk write error: {}", e);
                    self.stats.write_failure_count += 1;

                    // alert torrent of block write failure
                    self.alert_chan.send(TorrentAlert::BatchWrite(Err(e)))?;
                }
            }
        }

        Ok(())
    }

    /// Starts a new in-progress piece, creating metadata for it in self.
    ///
    /// This involves getting the expected hash of the piece, its length, and
    /// calculating the files that it intersects.
    fn start_new_piece(&mut self, info: BlockInfo) -> Result<(), WriteError> {
        log::trace!("Creating piece {} write buffer", info.piece_index);

        // get the position of the piece in the concatenated hash string
        let hash_pos = info.piece_index * 20;
        if hash_pos + 20 > self.piece_hashes.len() {
            log::warn!("Piece index {} is invalid", info.piece_index);
            return Err(WriteError::InvalidPieceIndex);
        }

        let hash_slice = &self.piece_hashes[hash_pos..hash_pos + 20];
        let mut expected_hash = [0; 20];
        expected_hash.copy_from_slice(hash_slice);
        log::debug!(
            "Piece {} expected hash {}",
            info.piece_index,
            hex::encode(&expected_hash)
        );

        // TODO: consider using expect here as piece index should be verified in
        // Torrent::write_block
        let len = self
            .info
            .piece_len(info.piece_index)
            .map_err(|_| WriteError::InvalidPieceIndex)?;
        log::debug!("Piece {} is {} bytes long", info.piece_index, len);

        let files = self
            .info
            .files_intersecting_piece(info.piece_index)
            .map_err(|_| WriteError::InvalidPieceIndex)?;
        log::debug!("Piece {} intersects files: {:?}", info.piece_index, files);

        let piece = Piece {
            expected_hash,
            len,
            blocks: BTreeMap::new(),
            files,
        };
        self.pieces.insert(info.piece_index, piece);

        Ok(())
    }
}

struct TorrentFile {
    info: FileInfo,
    handle: File,
}

impl TorrentFile {
    /// Opens the file in create, read, and write modes at the path of combining the
    /// download directory and the path defined in the file info.
    fn new(
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
    /// It returns the slice of blocks that weren't written to disk, that is,
    /// it returns the second half of `blocks` as though they were split at the
    /// `slice.len` offset (or an empty slice if all blocks were written to disk).
    ///
    /// # Important
    ///
    /// Since the syscall may be invoked repeatedly to perform the write, this
    /// means that the file write is not guaranteed to be atomic.
    fn write_blocks<'a>(
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

        // IO syscalls are not guaranteed to write the whole input buffer in one
        // go, so we need to write until all bytes have been confirmed to be
        // written to disk (or an error occurs)
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
            // written
            iovecs.advance(write_count);
        }

        Ok(iovecs.into_tail())
    }
}

#[derive(Default)]
struct Stats {
    /// The number of bytes successfully written to disk.
    write_count: u64,
    /// The number of times we failed to write to disk.
    write_failure_count: usize,
}

/// An in-progress piece download that keeps in memory the so far downloaded
/// blocks and the expected hash of the piece.
struct Piece {
    /// The expected hash of the whole piece.
    expected_hash: Sha1Hash,
    /// The length of the piece, in bytes.
    len: u32,
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
    blocks: BTreeMap<u32, Vec<u8>>,
    /// The files that this piece overlaps with.
    ///
    /// This is a left-inclusive range of all all file indices, that can be used
    /// to index the `Torrent::files` vector to get the file handles.
    files: Range<FileIndex>,
}

impl Piece {
    /// Places block into piece's write buffer if it doesn't exist. TODO: should
    /// we return an error if it does?
    fn enqueue_block(&mut self, offset: u32, data: Vec<u8>) {
        if self.blocks.contains_key(&offset) {
            log::warn!("Duplicate piece block at offset {}", offset);
        } else {
            self.blocks.insert(offset, data);
        }
    }

    /// Returns true if the piece has all its blocks in its write buffer.
    fn is_complete(&self) -> bool {
        self.blocks.len() == block_count(self.len)
    }

    /// Calculates the piece's hash using all its blocks and returns if it
    /// matches the expected hash.
    ///
    /// # Important
    ///
    /// This is potentially a computationally expensive function and should be
    /// executed on a thread pool and not the executor.
    fn matches_hash(&self) -> bool {
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
    fn write(
        &self,
        piece_torrent_offset: u64,
        files: &[Mutex<TorrentFile>],
    ) -> Result<(), WriteError> {
        let mut total_write_count = 0;

        // need to convert the blocks to IO slices that the underlying
        // systemcall can deal with
        let mut blocks: Vec<_> = self
            .blocks
            .values()
            .map(|b| IoVec::from_slice(&b))
            .collect();
        let mut bufs = blocks.as_mut_slice();
        // the offset at which we need to write in torrent, which is updated
        // with each write
        let mut write_torrent_offset = piece_torrent_offset;

        // loop through all files piece overlaps with and write that part of
        // piece to file
        let files = &files[self.files.clone()];
        debug_assert!(!files.is_empty());
        // TODO: optimize here for single file IO: no need to perform the splitting
        // buffers etc if we know there is only a single file that piece spans,
        // we can just write all blocks to that file
        for file in files.iter() {
            let file = file.lock().unwrap();
            // determine which part of the file we need to write to
            debug_assert!(self.len as u64 > total_write_count);
            let remaining_piece_len = self.len as u64 - total_write_count;
            let file_slice = file
                .info
                .get_slice(write_torrent_offset, remaining_piece_len);
            // an empty file slice shouldn't occur as it would mean that piece
            // was thought to span fewer files than it actually does
            debug_assert!(file_slice.len > 0);
            // the write buffer should still contain bytes to write
            debug_assert!(!bufs.is_empty());
            debug_assert!(!bufs[0].as_slice().is_empty());

            // write to file
            let tail = file.write_blocks(file_slice, bufs)?;

            // `write_vectored_at` only writes at most `slice.len` bytes of
            // `bufs` to disk and returns the portion that wasn't
            // written, which we can use to set the write buffer for the next
            // round
            bufs = tail;

            write_torrent_offset += file_slice.len as u64;
            total_write_count += file_slice.len;
        }

        // we should have used up all write buffers (i.e. written all blocks to
        // disk)
        debug_assert!(bufs.is_empty());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Read, path::PathBuf};

    use {super::*, crate::BLOCK_LEN};

    const DOWNLOAD_DIR: &str = "/tmp";

    // Tests that writing blocks to a single file using `TorrentFile` works.
    #[test]
    fn should_write_blocks_to_torrent_file() {
        let files = 0..1;
        let piece = make_piece(files);

        let download_dir = Path::new(DOWNLOAD_DIR);
        let mut file = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("TorrentFile_write_block.test"),
                torrent_offset: 0,
                len: 2 * piece.len as u64,
            },
        )
        .expect("couldn't create test file");

        // write buffers
        let file_slice = file.info.get_slice(0, piece.len as u64);
        let mut iovecs: Vec<_> = piece
            .blocks
            .values()
            .map(|b| IoVec::from_slice(&b))
            .collect();
        let tail = file
            .write_blocks(file_slice, &mut iovecs)
            .expect("couldn't write piece to file");
        assert!(tail.is_empty(), "not all blocks were written to disk");

        // read and compare
        let mut file_content = Vec::new();
        file.handle
            .read_to_end(&mut file_content)
            .expect("couldn't read test file");
        assert_eq!(
            file_content,
            piece.blocks.values().cloned().flatten().collect::<Vec<_>>(),
            "file content does not equal piece"
        );

        // clean up env
        fs::remove_file(download_dir.join(&file.info.path))
            .expect("couldn't remove test file");
    }

    // Tests that writing piece to a single file works.
    #[test]
    fn should_write_piece_to_single_file() {
        let files = 0..1;
        let piece = make_piece(files);
        let download_dir = Path::new(DOWNLOAD_DIR);
        let file = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_single_file.test"),
                torrent_offset: 0,
                len: 2 * piece.len as u64,
            },
        )
        .expect("couldn't create test file");
        let files = &[Mutex::new(file)];

        // piece starts at the beginning of files
        let piece_torrent_offset = 0;
        piece
            .write(piece_torrent_offset, files)
            .expect("Could not write piece to file");

        // compare file content to piece
        let mut file = files[0].lock().unwrap();
        let mut file_content = Vec::new();
        file.handle
            .read_to_end(&mut file_content)
            .expect("couldn't read test file");
        assert_eq!(
            file_content,
            piece.blocks.values().cloned().flatten().collect::<Vec<_>>(),
            "file {:?} content does not equal piece",
            file.info
        );

        // clean up env
        fs::remove_file(download_dir.join(&file.info.path))
            .expect("couldn't remove test file");
    }

    // Tests that writing piece to multiple files works.
    #[test]
    fn should_write_piece_to_multiple_files() {
        // piece spans 3 files
        let files = 0..3;
        let piece = make_piece(files);
        let download_dir = Path::new(DOWNLOAD_DIR);
        let file1 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files1.test"),
                torrent_offset: 0,
                len: BLOCK_LEN as u64 + 3,
            },
        )
        .expect("couldn't create test file 1");
        let file2 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files2.test"),
                torrent_offset: file1.info.len,
                len: BLOCK_LEN as u64 - 1500,
            },
        )
        .expect("couldn't create test file 2");
        let file3 = TorrentFile::new(
            download_dir,
            FileInfo {
                path: PathBuf::from("Piece_write_files3.test"),
                torrent_offset: file2.info.torrent_offset + file2.info.len,
                len: piece.len as u64 - (file1.info.len + file2.info.len),
            },
        )
        .expect("couldn't create test file 3");
        let files = &[Mutex::new(file1), Mutex::new(file2), Mutex::new(file3)];

        // piece starts at the beginning of files
        let piece_torrent_offset = 0;
        piece
            .write(piece_torrent_offset, files)
            .expect("Could not write piece to file");

        // compare contents of files to piece
        for file in files.iter() {
            let mut file = file.lock().unwrap();
            let mut file_content = Vec::new();
            file.handle
                .read_to_end(&mut file_content)
                .expect("couldn't read test file");
            // compare the content of file to the portion that corresponds to
            // piece
            assert_eq!(
                file_content,
                piece
                    .blocks
                    .values()
                    .cloned()
                    .flatten()
                    .skip(file.info.torrent_offset as usize)
                    .take(file.info.len as usize)
                    .collect::<Vec<_>>(),
                "file {:?} content does not equal piece",
                file.info
            );
        }

        // clean up env
        for file in files.iter() {
            let path = download_dir.join(&file.lock().unwrap().info.path);
            fs::remove_file(path).expect("couldn't remove test file");
        }
    }

    // Creates a piece for testing that has 4 blocks of length `BLOCK_LEN`.
    fn make_piece(files: Range<FileIndex>) -> Piece {
        let blocks = vec![
            (0 * BLOCK_LEN..1 * BLOCK_LEN)
                .map(|b| b % u8::MAX as u32)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
            (1 * BLOCK_LEN..2 * BLOCK_LEN)
                .map(|b| b % u8::MAX as u32)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
            (2 * BLOCK_LEN..3 * BLOCK_LEN)
                .map(|b| b % u8::MAX as u32)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
            (3 * BLOCK_LEN..4 * BLOCK_LEN)
                .map(|b| b % u8::MAX as u32)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
        ];
        let expected_hash = {
            let mut hasher = Sha1::new();
            for block in blocks.iter() {
                hasher.input(&block);
            }
            hasher.result().into()
        };
        let len = blocks.len() as u32 * BLOCK_LEN;
        // convert blocks to a b-tree map
        let (blocks, _) = blocks.into_iter().fold(
            (BTreeMap::new(), 0u32),
            |(mut map, mut offset), block| {
                let block_len = block.len();
                map.insert(offset, block);
                offset += block_len as u32;
                (map, offset)
            },
        );
        Piece {
            expected_hash,
            len,
            blocks,
            files,
        }
    }
}
