mod error;
mod io;

pub use error::*;

use {
    crate::{BlockInfo, TorrentId},
    io::Disk,
    std::path::PathBuf,
    tokio::{
        sync::mpsc::{UnboundedReceiver, UnboundedSender},
        task,
    },
};

/// Spawns a disk IO task and returns a tuple with the task join handle, the
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
    /// Instructs the disk task to set up everything needed for a new torrent,
    /// which includes in-memory metadata storage and pre-allocating the
    /// to-be-downloaded file(s).
    pub fn allocate_new_torrent(
        &self,
        id: TorrentId,
        download_path: PathBuf,
        piece_hashes: Vec<u8>,
        piece_count: usize,
        piece_len: u32,
        last_piece_len: u32,
    ) -> Result<()> {
        log::trace!("Allocating new torrent {}", id);
        self.0
            .send(Command::NewTorrent {
                id,
                download_path,
                piece_hashes,
                piece_count,
                piece_len,
                last_piece_len,
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
        info: BlockInfo,
        data: Vec<u8>,
    ) -> Result<()> {
        log::trace!("Saving block {:?} to disk", info);
        self.0
            .send(Command::WriteBlock { id, info, data })
            .map_err(Error::from)
    }

    /// Shuts down the disk IO task.
    pub fn shutdown(&self) -> Result<()> {
        log::trace!("Shutting down disk IO task");
        self.0.send(Command::Shutdown).map_err(Error::from)
    }
}

// The channel for sendng commands to the disk task.
type CommandSender = UnboundedSender<Command>;
// The channel the disk task uses to listen for commands.
type CommandReceiver = UnboundedReceiver<Command>;

// The type of commands that the disk can execute.
enum Command {
    // Allocate a new torrent.
    NewTorrent {
        id: TorrentId,
        download_path: PathBuf,
        piece_hashes: Vec<u8>,
        piece_count: usize,
        piece_len: u32,
        last_piece_len: u32,
    },
    // Request to eventually write a block to disk.
    WriteBlock {
        id: TorrentId,
        info: BlockInfo,
        data: Vec<u8>,
    },
    // Eventually shut down the disk task.
    Shutdown,
}

// The type of channel used to alert the engine about global events.
type AlertSender = UnboundedSender<Alert>;
/// The channel on which the engine can listen for global disk events.
pub(crate) type AlertReceiver = UnboundedReceiver<Alert>;

/// The alerts that the disk task may send about global events (i.e.  events not
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

// The type of channel used to alert a torrent about torrent specific events.
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
    pub blocks: Vec<BlockInfo>,
    /// This field is set for the block write that completes the piece and
    /// contains whether the downloaded piece's hash matches its expected hash.
    pub is_piece_valid: Option<bool>,
}
