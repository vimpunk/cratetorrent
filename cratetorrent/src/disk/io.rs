use {
    super::{
        error::*, Alert, AlertReceiver, AlertSender, BatchWrite, Command,
        CommandReceiver, CommandSender, TorrentAlert, TorrentAlertReceiver,
        TorrentAlertSender, TorrentAllocation,
    },
    crate::{block_count, BlockInfo, Sha1Hash, TorrentId},
    sha1::{Digest, Sha1},
    std::{
        collections::{BTreeMap, HashMap},
        fs::OpenOptions,
        io::{IoSlice, Write},
        path::{Path, PathBuf},
    },
    tokio::{
        sync::{mpsc, RwLock},
        task,
    },
};

// The entity responsible for saving downloaded file blocks to disk and
// verifying whether downloaded pieces are valid.
pub(super) struct Disk {
    // Each torrent in engine has a corresponding entry in this hashmap, which
    // includes various metadata about torrent and the torrent specific alert
    // channel.
    torrents: HashMap<TorrentId, RwLock<Torrent>>,
    // Port on which disk IO commands are received.
    cmd_port: CommandReceiver,
    // Channel on which `Disk` sends alerts to the torrent engine.
    alert_chan: AlertSender,
}

impl Disk {
    // Creates a new `Disk` instance and returns a command sender and an alert
    // receiver.
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

    // Starts the disk event loop which is run until shutdown or an
    // unrecoverable error occurs (e.g. mpsc channel failure).
    pub(super) async fn start(&mut self) -> Result<()> {
        log::info!("Starting disk IO event loop");
        while let Some(cmd) = self.cmd_port.recv().await {
            log::debug!("Disk received command");
            match cmd {
                Command::NewTorrent {
                    id,
                    download_path,
                    piece_hashes,
                    piece_count,
                    piece_len,
                    last_piece_len,
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
                    let torrent_res = Torrent::new(
                        download_path,
                        piece_hashes,
                        piece_count,
                        piece_len,
                        last_piece_len,
                    );
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

    // Queues a block for writing and fails if the torrent id is invalid.
    //
    // If the block could not be written due to IO failure, the torrent is
    // notified of it.
    async fn write_block(
        &self,
        id: TorrentId,
        info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        log::trace!("Saving torrent {} block {:?} to disk", id, info);

        // check torrent id
        let torrent = self.torrents.get(&id).ok_or_else(|| {
            log::warn!("Torrent {} not found", id);
            Error::InvalidTorrentId
        })?;
        torrent.write().await.write_block(info, data).await
    }
}

// Torrent information related to disk IO.
//
// Contains the in-progress pieces (i.e. the write buffer), metadata about
// torrent's download and piece sizes, etc.
struct Torrent {
    // The path where the file is saved.
    download_path: PathBuf,
    // The channel used to alert a torrent that a block has been written to
    // disk and/or a piece was completed.
    alert_chan: TorrentAlertSender,
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
    stats: Stats,
}

#[derive(Default)]
struct Stats {
    // The number of bytes successfully written to disk.
    write_count: u64,
    // The number of times we failed to write to disk.
    write_failure_count: usize,
}

impl Torrent {
    fn new(
        download_path: PathBuf,
        piece_hashes: Vec<u8>,
        piece_count: usize,
        piece_len: u32,
        last_piece_len: u32,
    ) -> Result<(Self, TorrentAlertReceiver), NewTorrentError> {
        // TODO: since this is done as part of a tokio::task, should we use
        // tokio_fs here?
        if download_path.exists() {
            log::warn!("Download path {:?} exists", download_path);
            return Err(NewTorrentError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Download path already exists",
            )));
        }

        let (alert_chan, alert_port) = mpsc::unbounded_channel();

        Ok((
            Self {
                download_path,
                alert_chan,
                pieces: HashMap::new(),
                piece_hashes,
                piece_count,
                piece_len,
                last_piece_len,
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

        if !self.pieces.contains_key(&info.piece_index) {
            if let Err(e) = self.start_new_piece(info) {
                self.alert_chan.send(TorrentAlert::BatchWrite(Err(e)))?;
                // return with ok as the disk task itself shouldn't be aborted
                // due to invalid input
                return Ok(());
            }
        }
        // TODO: don't unwrap here
        let piece = self
            .pieces
            .get_mut(&info.piece_index)
            .expect("Newly inserted piece not present");

        piece.enqueue_block(info.offset, data);

        // if the piece has all its blocks, it means we can hash it and save it
        // to disk and clear its write buffer
        if piece.is_complete() {
            // TODO: remove from in memory store only if the disk write
            // succeeded (otherwise we need to retry later)
            let piece = self.pieces.remove(&info.piece_index).unwrap();

            let download_path = self.download_path.clone();

            // don't block the reactor with the potentially expensive hashing
            // and sync file writing
            //
            // NOTE: Do _NOT_ return on disk write failure, we don't want to
            // kill the disk task due to potential disk IO errors (which may
            // happen from time to time). We need to alert torrent of this
            // failure and return normally.
            //
            // TODO(https://github.com/mandreyel/cratetorrent/issues/23): also
            // place back piece write buffer in torrent and retry later
            let write_result = task::spawn_blocking(move || {
                    let is_piece_valid = piece.matches_hash();

                    // save piece to disk if it's valid
                    let (write_count, blocks) = if is_piece_valid {
                        log::info!("Piece {} is valid", info.piece_index);
                        let write_count = piece.write(&download_path)?;

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

                        (Some(write_count), blocks)
                    } else {
                        log::warn!("Piece {} is NOT valid", info.piece_index);
                        (None, Vec::new())
                    };

                    Ok((is_piece_valid, write_count, blocks))
                })
                .await
                // our code doesn't panic in the task so until better strategies
                // are devised, unwrap
                .expect("disk IO write task panicked");

            match write_result {
                Ok((is_piece_valid, write_count, blocks)) => {
                    // record write statistics if the piece is valid
                    if is_piece_valid {
                        if let Some(write_count) = write_count {
                            self.stats.write_count += write_count as u64;
                        }
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

// An in-progress piece download that keeps in memory the so far downloaded
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
    // Places block into piece's write buffer if it doesn't exist. TODO: should
    // we return an error if it does?
    fn enqueue_block(&mut self, offset: u32, data: Vec<u8>) {
        if self.blocks.contains_key(&offset) {
            log::warn!("Duplicate piece block at offset {}", offset);
        } else {
            self.blocks.insert(offset, data);
        }
    }

    // Returns true if the piece has all its blocks in its write buffer.
    fn is_complete(&self) -> bool {
        self.blocks.len() == block_count(self.len)
    }

    // Calculates the piece's hash using all its blocks and returns if it
    // matches the expected hash.
    fn matches_hash(&self) -> bool {
        let mut hasher = Sha1::new();
        for block in self.blocks.values() {
            hasher.input(&block);
        }
        let hash = hasher.result();
        log::debug!("Piece hash: {:x}", hash);
        hash.as_slice() == self.expected_hash
    }

    // Writes the piece's write buffer to disk at the given path and returns the
    // number of bytes written.
    fn write(&self, part_path: &Path) -> Result<usize, WriteError> {
        let mut block_slices: Vec<_> =
            self.blocks.values().map(|b| IoSlice::new(&b[..])).collect();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(part_path)
            .map_err(|e| {
                log::warn!("Failed to open file {:?}", part_path);
                WriteError::Io(e)
            })?;
        // IO syscalls are not guaranteed to write the whole input buffer in one
        // go, so we need to write until all bytes have been confirmed to be
        // written to disk (or an error occurs)
        let mut total_write_count = 0;
        while total_write_count < self.len as usize {
            let write_count =
                file.write_vectored(&block_slices).map_err(|e| {
                    log::warn!("Failed to write to file {:?}", part_path);
                    WriteError::Io(e)
                })?;
            // "trim off" the bytes from the block slices that were written to
            // disk so that they are not written there again
            IoSlice::advance(&mut block_slices, write_count);
            total_write_count += write_count;
        }

        Ok(total_write_count)
    }
}
