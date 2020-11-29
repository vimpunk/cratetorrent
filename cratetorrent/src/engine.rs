use std::{net::SocketAddr, path::PathBuf};

use tokio::runtime::Runtime;

use crate::{
    disk::{self, Alert},
    error::*,
    metainfo::Metainfo,
    storage_info::StorageInfo,
    torrent::Torrent,
    tracker::Tracker,
    Bitfield, PeerId,
};

/// Connects to seed peers and downloads the torrent.
pub fn download_torrent(
    client_id: PeerId,
    download_dir: PathBuf,
    metainfo: Metainfo,
    listen_addr: SocketAddr,
    seeds: Vec<SocketAddr>,
) -> Result<()> {
    if seeds.is_empty() {
        log::warn!("List of seeds is empty, cannot download torrent");
        return Ok(());
    }

    let mut rt = Runtime::new()?;
    rt.block_on(start_engine(
        client_id,
        download_dir,
        metainfo,
        listen_addr,
        Mode::Download { seeds },
    ))
}

/// Seeds a torrent until an error occurs.
pub fn seed_torrent(
    client_id: PeerId,
    download_dir: PathBuf,
    metainfo: Metainfo,
    listen_addr: SocketAddr,
) -> Result<()> {
    let mut rt = Runtime::new()?;
    rt.block_on(start_engine(
        client_id,
        download_dir,
        metainfo,
        listen_addr,
        Mode::Seed,
    ))
}

enum Mode {
    Download { seeds: Vec<SocketAddr> },
    Seed,
}

impl Mode {
    fn own_pieces(&self, piece_count: usize) -> Bitfield {
        match self {
            Self::Download { .. } => Bitfield::repeat(false, piece_count),
            Self::Seed => Bitfield::repeat(true, piece_count),
        }
    }

    fn seeds(self) -> Vec<SocketAddr> {
        match self {
            Self::Download { seeds } => seeds,
            _ => Vec::new(),
        }
    }
}

async fn start_engine(
    client_id: PeerId,
    download_dir: PathBuf,
    metainfo: Metainfo,
    listen_addr: SocketAddr,
    mode: Mode,
) -> Result<()> {
    let (disk_join_handle, disk, mut alert_port) = disk::spawn()?;

    // allocate torrent on disk
    let id = 0;
    let info_hash = metainfo.info_hash;
    let storage_info = StorageInfo::new(&metainfo, download_dir);
    log::info!("Torrent {} storage info: {:?}", id, storage_info);

    // allocate torrent and wait for its result
    disk.allocate_new_torrent(id, storage_info.clone(), metainfo.pieces)?;
    let torrent_disk_alert_port =
        if let Some(Alert::TorrentAllocation(allocation_result)) =
            alert_port.recv().await
        {
            match allocation_result {
                Ok(allocation) => {
                    log::info!("Torrent {} allocated on disk", id);
                    debug_assert_eq!(allocation.id, id);
                    allocation.alert_port
                }
                Err(e) => {
                    log::error!(
                        "Torrent {} could not be allocated on disk: {}",
                        id,
                        e
                    );
                    return Ok(());
                }
            }
        } else {
            log::error!(
                "Disk task receive error, disk task most likely stopped"
            );
            return Ok(());
        };

    let mut trackers = Vec::with_capacity(metainfo.trackers.len());
    for tracker_url in metainfo.trackers.into_iter() {
        trackers.push(Tracker::new(tracker_url));
    }

    let own_pieces = mode.own_pieces(storage_info.piece_count);
    let mut torrent = Torrent::new(
        id,
        disk.clone(),
        torrent_disk_alert_port,
        info_hash,
        storage_info,
        own_pieces,
        trackers,
        client_id,
        listen_addr,
    );
    let seeds = mode.seeds();
    torrent.start(&seeds).await?;

    // send a shutdown command to disk
    disk.shutdown()?;
    // and join on its handle
    disk_join_handle
        .await
        .expect("Disk task has panicked")
        .map_err(Error::from)?;

    Ok(())
}
