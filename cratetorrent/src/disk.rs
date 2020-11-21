use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task,
};

use crate::{
    error::Error, peer, storage_info::StorageInfo, BlockInfo, TorrentId,
};
use io::Disk;

pub(crate) use error::*;

mod error;
mod io;

// Spawns a disk IO task and returns a tuple with the task join handle, the
/// disk handle used for sending commands, and a channel for receiving
/// command results and other notifications.
pub(crate) fn spawn(
) -> Result<(task::JoinHandle<Result<()>>, DiskHandle, AlertReceiver)> {
    log::info!("Spawning disk IO task");
    let (mut disk, cmd_chan, alert_port) = Disk::new()?;
    // spawn disk event loop on a new task
    let join_handle = task::spawn(async move { disk.start().await });
    log::info!("Spawned disk IO task");

    Ok((join_handle, DiskHandle(cmd_chan), alert_port))
}

/// The handle for the disk task, used to execute disk IO related tasks.
///
/// The handle may be copied an arbitrary number of times. It is an abstraction
/// over the means to communicate with the disk IO task. For now, mpsc channels
/// are used for issuing commands and receiving results, but this may well
/// change later on, hence hiding this behind this handle type.
#[derive(Clone)]
pub(crate) struct DiskHandle(CommandSender);

impl DiskHandle {
    /// Creates a new torrent in the disk task.
    ///
    /// This instructs the disk task to set up everything needed for a new
    /// torrent, which includes in-memory metadata storage and setting up the
    /// torrent's file system structure on disk.
    pub fn allocate_new_torrent(
        &self,
        id: TorrentId,
        storage_info: StorageInfo,
        piece_hashes: Vec<u8>,
    ) -> Result<()> {
        log::trace!("Allocating new torrent {}", id);
        self.0
            .send(Command::NewTorrent {
                id,
                storage_info,
                piece_hashes,
            })
            .map_err(Error::from)
    }

    /// Queues a block for eventual writing to disk.
    ///
    /// Once the block is saved, the result is advertised to its
    /// `AlertReceiver`.
    pub fn write_block(
        &self,
        id: TorrentId,
        block_info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        log::trace!("Saving block {:?} to disk", block_info);
        self.0
            .send(Command::WriteBlock {
                id,
                block_info,
                data,
            })
            .map_err(Error::from)
    }

    /// Issues a request for a block on the disk.
    ///
    /// Once the block is saved, the result is advertised to its
    /// `AlertReceiver`.
    pub fn read_block(
        &self,
        id: TorrentId,
        block_info: BlockInfo,
        result_chan: peer::Sender,
    ) -> Result<()> {
        log::trace!("Reading block {:?} from disk", block_info);
        self.0
            .send(Command::ReadBlock {
                id,
                block_info,
                result_chan,
            })
            .map_err(Error::from)
    }

    /// Shuts down the disk IO task.
    pub fn shutdown(&self) -> Result<()> {
        log::trace!("Shutting down disk IO task");
        self.0.send(Command::Shutdown).map_err(Error::from)
    }
}

/// The channel for sendng commands to the disk task.
type CommandSender = UnboundedSender<Command>;
/// The channel the disk task uses to listen for commands.
type CommandReceiver = UnboundedReceiver<Command>;

/// The type of commands that the disk can execute.
enum Command {
    /// Allocate a new torrent in `Disk`.
    NewTorrent {
        id: TorrentId,
        storage_info: StorageInfo,
        piece_hashes: Vec<u8>,
    },
    /// Request to eventually write a block to disk.
    WriteBlock {
        id: TorrentId,
        block_info: BlockInfo,
        data: Vec<u8>,
    },
    /// Request to eventually read a block from disk and return it via the
    /// sender.
    ReadBlock {
        id: TorrentId,
        block_info: BlockInfo,
        result_chan: peer::Sender,
    },
    /// Eventually shut down the disk task.
    Shutdown,
}

/// The type of channel used to alert the engine about global events.
type AlertSender = UnboundedSender<Alert>;
/// The channel on which the engine can listen for global disk events.
pub(crate) type AlertReceiver = UnboundedReceiver<Alert>;

/// The alerts that the disk task may send about global events (i.e. events not
/// related to individual torrents).
#[derive(Debug)]
pub(crate) enum Alert {
    /// Torrent allocation result. If successful, the id of the allocated
    /// torrent is returned for identification, if not, the reason of the error
    /// is included.
    TorrentAllocation(Result<TorrentAllocation, NewTorrentError>),
}

/// The result of successfully allocating a torrent.
#[derive(Debug)]
pub(crate) struct TorrentAllocation {
    /// The id of the torrent that has been allocated.
    pub id: TorrentId,
    /// The port on which torrent may receive alerts.
    pub alert_port: TorrentAlertReceiver,
}

/// The type of channel used to alert a torrent about torrent specific events.
type TorrentAlertSender = UnboundedSender<TorrentAlert>;
/// The type of channel on which a torrent can listen for block write
/// completions.
pub(crate) type TorrentAlertReceiver = UnboundedReceiver<TorrentAlert>;

/// The alerts that the disk task may send about events related to a specific
/// torrent.
#[derive(Debug)]
pub(crate) enum TorrentAlert {
    /// Sent when some blocks were written to disk or an error ocurred while
    /// writing.
    BatchWrite(Result<BatchWrite, WriteError>),
    /// There was an error reading a block.
    ReadError {
        block_info: BlockInfo,
        error: ReadError,
    },
}

/// Type returned on each successful batch of blocks written to disk.
#[derive(Debug)]
pub(crate) struct BatchWrite {
    /// The piece blocks that were written to the disk in this batch.
    ///
    /// There is some inefficiency in having the piece index in all blocks,
    /// however, this allows for supporting alerting writes of blocks in
    /// multiple pieces, which is a feature for later (and for now this is kept
    /// for simplicity).
    ///
    /// If the piece is invalid, this vector is empty.
    pub blocks: Vec<BlockInfo>,
    /// This field is set for the block write that completes the piece and
    /// contains whether the downloaded piece's hash matches its expected hash.
    pub is_piece_valid: Option<bool>,
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use sha1::{Digest, Sha1};
    use tokio::sync::mpsc;

    use super::*;
    use crate::{block_count, storage_info::FsStructure, FileInfo, BLOCK_LEN};

    /// Tests the allocation of a torrent, and then the allocation of the same
    /// torrent returning an error.
    #[tokio::test]
    async fn should_allocate_new_torrent() {
        let (_, disk_handle, mut alert_port) = spawn().unwrap();

        let Env {
            id,
            piece_hashes,
            info,
            ..
        } = Env::new("allocate_new_torrent");

        // allocate torrent via channel
        disk_handle
            .allocate_new_torrent(id, info.clone(), piece_hashes.clone())
            .unwrap();

        // wait for result on alert port
        let alert = alert_port.recv().await.unwrap();
        match alert {
            Alert::TorrentAllocation(res) => {
                assert!(res.is_ok());
                assert_eq!(res.unwrap().id, id);
            }
        }

        // check that file was created on disk
        let file = match &info.structure {
            FsStructure::File(file) => file,
            _ => unreachable!(),
        };
        assert!(info.download_dir.join(&file.path).is_file());

        // try to allocate the same torrent a second time
        disk_handle
            .allocate_new_torrent(id, info, piece_hashes)
            .unwrap();

        // we should get an already exists error
        let alert = alert_port.recv().await.unwrap();
        assert!(matches!(
            alert,
            Alert::TorrentAllocation(Err(NewTorrentError::AlreadyExists))
        ));
    }

    /// Tests writing of a complete valid torrent's pieces and verifying that an
    /// alert of each disk write is returned by the disk task.
    #[tokio::test]
    async fn should_write_all_pieces() {
        let (_, disk_handle, mut alert_port) = spawn().unwrap();

        let Env {
            id,
            pieces,
            piece_hashes,
            info,
        } = Env::new("write_all_pieces");

        // allocate torrent via channel
        disk_handle
            .allocate_new_torrent(id, info.clone(), piece_hashes)
            .unwrap();

        // wait for result on alert port
        let mut torrent_disk_alert_port =
            if let Some(Alert::TorrentAllocation(Ok(allocation))) =
                alert_port.recv().await
            {
                allocation.alert_port
            } else {
                assert!(false, "torrent could not be allocated");
                return;
            };

        // write all pieces to disk
        for index in 0..pieces.len() {
            let piece = &pieces[index];
            for_each_block(index, piece.len() as u32, |block| {
                let block_end = block.offset + block.len;
                let data = &piece[block.offset as usize..block_end as usize];
                debug_assert_eq!(data.len(), block.len as usize);
                println!("Writing piece {} block {:?}", index, block);
                disk_handle.write_block(id, block, data.to_vec()).unwrap();
            });

            // wait for disk write result
            if let Some(TorrentAlert::BatchWrite(Ok(batch))) =
                torrent_disk_alert_port.recv().await
            {
                // piece is complete so it should be hashed and be valid
                assert_eq!(batch.is_piece_valid, Some(true));
                // verify that the message contains all four blocks
                for_each_block(index, piece.len() as u32, |block| {
                    let pos = batch.blocks.iter().position(|b| *b == block);
                    println!("Verifying piece {} block {:?}", index, block);
                    assert!(pos.is_some());
                });
            } else {
                assert!(false, "Piece could not be written to disk");
            }
        }

        // clean up test env
        let file = match &info.structure {
            FsStructure::File(file) => file,
            _ => unreachable!(),
        };
        fs::remove_file(info.download_dir.join(&file.path))
            .expect("Failed to clean up disk test torrent file");
    }

    /// Tests writing of an invalid piece and verifying that an alert of it
    /// is returned by the disk task.
    #[tokio::test]
    async fn should_reject_writing_invalid_piece() {
        let (_, disk_handle, mut alert_port) = spawn().unwrap();

        let Env {
            id,
            pieces,
            piece_hashes,
            info,
        } = Env::new("write_invalid_piece");

        // allocate torrent via channel
        disk_handle
            .allocate_new_torrent(id, info.clone(), piece_hashes)
            .unwrap();

        // wait for result on alert port
        let mut torrent_disk_alert_port =
            if let Some(Alert::TorrentAllocation(Ok(allocation))) =
                alert_port.recv().await
            {
                allocation.alert_port
            } else {
                assert!(false, "torrent could not be allocated");
                return;
            };

        // write an invalid piece to disk
        let index = 0;
        let invalid_piece: Vec<_> =
            pieces[index].iter().map(|b| b.saturating_add(5)).collect();
        for_each_block(index, invalid_piece.len() as u32, |block| {
            let block_end = block.offset + block.len;
            let data =
                &invalid_piece[block.offset as usize..block_end as usize];
            debug_assert_eq!(data.len(), block.len as usize);
            println!("Writing invalid piece {} block {:?}", index, block);
            disk_handle.write_block(id, block, data.to_vec()).unwrap();
        });

        // wait for disk write result
        if let Some(TorrentAlert::BatchWrite(Ok(batch))) =
            torrent_disk_alert_port.recv().await
        {
            // piece is complete so it should be hashed but be invalid
            assert_eq!(batch.is_piece_valid, Some(false));
            // verify that the message doesn't contain any blocks
            assert!(batch.blocks.is_empty());
        } else {
            assert!(false, "piece could not be written to disk");
        }
    }

    /// Tests writing of a complete valid torrent's pieces and verifying that an
    /// alert of each disk write is returned by the disk task.
    #[tokio::test]
    async fn should_read_piece_blocks() {
        let (_, disk_handle, mut alert_port) = spawn().unwrap();

        let Env {
            id,
            pieces,
            piece_hashes,
            info,
        } = Env::new("read_piece_blocks");

        // allocate torrent via channel
        disk_handle
            .allocate_new_torrent(id, info.clone(), piece_hashes)
            .unwrap();

        // wait for result on alert port
        let mut torrent_disk_alert_port =
            if let Some(Alert::TorrentAllocation(Ok(allocation))) =
                alert_port.recv().await
            {
                allocation.alert_port
            } else {
                assert!(false, "torrent could not be allocated");
                return;
            };

        // write second piece to disk
        let index = 1;
        let piece = &pieces[index];
        for_each_block(index, piece.len() as u32, |block| {
            let block_end = block.offset + block.len;
            let data = &piece[block.offset as usize..block_end as usize];
            debug_assert_eq!(data.len(), block.len as usize);
            println!("Writing piece {} block {:?}", index, block);
            disk_handle.write_block(id, block, data.to_vec()).unwrap();
        });

        // wait for disk write result
        assert!(torrent_disk_alert_port.recv().await.is_some());

        // set up channels to communicate disk read results
        let (chan, mut port) = mpsc::unbounded_channel();

        // read each block in piece
        let block_count = block_count(piece.len() as u32) as u32;
        let mut block_offset = 0u32;
        for _ in 0..block_count {
            // when calculating the block length we need to consider that the
            // last block may be smaller than the rest
            let block_len = (piece.len() as u32 - block_offset).min(BLOCK_LEN);
            let block_info = BlockInfo {
                piece_index: index,
                offset: block_offset,
                len: block_len,
            };
            // read block
            disk_handle
                .read_block(id, block_info, chan.clone())
                .unwrap();

            // wait for result
            if let Some(peer::Command::Block { info, data }) = port.recv().await
            {
                assert_eq!(info, block_info);
            } else {
                assert!(false, "block could not be read from disk");
            }

            // increment offset for next piece
            block_offset += block_len;
        }

        // clean up test env
        let file = match &info.structure {
            FsStructure::File(file) => file,
            _ => unreachable!(),
        };
        fs::remove_file(info.download_dir.join(&file.path))
            .expect("Failed to clean up disk test torrent file");
    }

    /// Calls the provided function for each block in piece, passing it the
    /// block's `BlockInfo`.
    fn for_each_block(
        piece_index: usize,
        piece_len: u32,
        block_visitor: impl Fn(BlockInfo),
    ) {
        let block_count = block_count(piece_len) as u32;
        // all pieces have four blocks in this test
        debug_assert_eq!(block_count, 4);

        let mut block_offset = 0;
        for _ in 0..block_count {
            // when calculating the block length we need to consider that the
            // last block may be smaller than the rest
            let block_len = (piece_len - block_offset).min(BLOCK_LEN);
            debug_assert!(block_len > 0);
            debug_assert!(block_len <= BLOCK_LEN);

            block_visitor(BlockInfo {
                piece_index,
                offset: block_offset,
                len: block_len,
            });

            // increment offset for next piece
            block_offset += block_len;
        }
    }

    /// The disk IO test environment containing information of a valid torrent.
    struct Env {
        id: TorrentId,
        pieces: Vec<Vec<u8>>,
        piece_hashes: Vec<u8>,
        info: StorageInfo,
    }

    impl Env {
        /// Creates a new test environment.
        ///
        /// Tests are run in parallel so multiple environments must not clash,
        /// therefore the test name must be unique, which is included in the
        /// test environment's path. This also helps debugging.
        fn new(test_name: &str) -> Self {
            let id = 0;
            let download_dir = Path::new("/tmp");
            let download_rel_path =
                PathBuf::from(format!("torrent_disk_test_{}", test_name));
            let piece_len: u32 = 4 * 0x4000;
            // last piece is slightly shorter, to test that it is handled
            // correctly
            let last_piece_len: u32 = piece_len - 935;
            let pieces: Vec<Vec<u8>> = vec![
                (0..piece_len).map(|b| (b % 256) as u8).collect(),
                (0..piece_len)
                    .map(|b| b + 1)
                    .map(|b| (b % 256) as u8)
                    .collect(),
                (0..piece_len)
                    .map(|b| b + 2)
                    .map(|b| (b % 256) as u8)
                    .collect(),
                (0..last_piece_len)
                    .map(|b| b + 3)
                    .map(|b| (b % 256) as u8)
                    .collect(),
            ];
            // build up expected piece hashes
            let mut piece_hashes = Vec::with_capacity(pieces.len() * 20);
            for piece in pieces.iter() {
                let hash = Sha1::digest(&piece);
                piece_hashes.extend(hash.as_slice());
            }
            assert_eq!(piece_hashes.len(), pieces.len() * 20);

            // clean up any potential previous test env
            {
                let download_path = download_dir.join(&download_rel_path);
                if download_path.is_file() {
                    fs::remove_file(&download_path).expect(
                        "Failed to clean up previous disk test torrent file",
                    );
                } else if download_path.is_dir() {
                    fs::remove_dir_all(&download_path).expect(
                        "Failed to clean up previous disk test torrent dir",
                    );
                }
            }

            let download_len = pieces.iter().fold(0, |mut len, piece| {
                len += piece.len() as u64;
                len
            });
            let info = StorageInfo {
                piece_count: pieces.len(),
                piece_len,
                last_piece_len,
                download_len,
                download_dir: download_dir.to_path_buf(),
                structure: FsStructure::File(FileInfo {
                    path: download_rel_path,
                    torrent_offset: 0,
                    len: download_len,
                }),
            };

            Self {
                id,
                pieces,
                piece_hashes,
                info,
            }
        }
    }
}
