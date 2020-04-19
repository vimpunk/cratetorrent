use {
    crate::{
        disk::{self, Alert},
        error::*,
        metainfo::Metainfo,
        torrent::Torrent,
        PeerId,
    },
    std::{net::SocketAddr, path::Path},
    tokio::runtime::Runtime,
};

/// Connects to a single seed and downloads the torrent or aborts on error.
pub fn run_torrent(
    client_id: PeerId,
    download_dir: &Path,
    metainfo: Metainfo,
    seed_addr: SocketAddr,
) -> Result<()> {
    let mut rt = Runtime::new()?;
    rt.block_on(async move {
        start_disk_and_torrent(client_id, download_dir, metainfo, seed_addr)
            .await
    })
}

async fn start_disk_and_torrent(
    client_id: PeerId,
    download_dir: &Path,
    metainfo: Metainfo,
    seed_addr: SocketAddr,
) -> Result<()> {
    let (disk_join_handle, disk, mut alert_port) = disk::spawn()?;

    // allocate torrent on disk
    let id = 0;
    let download_path = download_dir.join(&metainfo.info.name);
    log::info!("Torrent {} download path: {:?}", id, download_path);

    let download_len = metainfo.download_len()?;
    log::info!("Torrent {} total length: {}", id, download_len);

    let piece_count = metainfo.piece_count();
    log::info!("Torrent {} piece count: {}", id, piece_count);

    let piece_len = metainfo.info.piece_len as u32;
    log::info!("Torrent {} piece length: {}", id, piece_len);

    let last_piece_len =
        download_len - piece_len as u64 * (piece_count - 1) as u64;
    let last_piece_len = last_piece_len as u32;
    log::info!("Torrent {} last piece length: {}", id, last_piece_len);

    disk.allocate_new_torrent(
        id,
        download_path,
        metainfo.info.pieces.clone(),
        piece_count,
        piece_len,
        last_piece_len,
    )?;

    // wait for torrent allocation result
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

    let mut torrent = Torrent::new(
        id,
        disk.clone(),
        torrent_disk_alert_port,
        client_id,
        metainfo,
        seed_addr,
    )?;
    // run torrent to completion
    torrent.start().await?;

    // send a shutdown command to disk
    disk.shutdown()?;
    // and join on its handle
    disk_join_handle
        .await
        .expect("Disk task has panicked")
        .map_err(Error::from)?;

    Ok(())
}
