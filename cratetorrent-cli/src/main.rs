use std::{fs, net::SocketAddr};

use clap::{App, Arg};
use cratetorrent::prelude::*;
use futures::stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // set up cli args
    let matches = App::new("Cratetorrent CLI")
        .version("1.0.0-alpha.1")
        .arg(
            Arg::with_name("listen")
                .short("l")
                .long("listen")
                .value_name("SOCKET_ADDR")
                .help(
                    "The socket address on which to listen for new connections",
                )
                .takes_value(true),
        )
        .arg(
            Arg::with_name("mode")
                .short("m")
                .long("mode")
                .value_name("MODE")
                .help("Whether to 'seed' or 'download' the torrent.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("seeds")
                .short("s")
                .long("seeds")
                .value_name("SOCKET_ADDRS")
                .help(
                    "A comma separated list of <ip>:<port> pairs of the seeds",
                )
                .takes_value(true),
        )
        .arg(
            Arg::with_name("metainfo")
                .short("i")
                .long("metainfo")
                .value_name("PATH")
                .help("The path to the torrent metainfo file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("download-dir")
                .short("d")
                .long("download-dir")
                .value_name("PATH")
                .help("The path of the folder where to download file")
                .takes_value(true),
        )
        .get_matches();

    // parse cli args

    // mandatory
    let listen_addr = matches.value_of("listen");
    println!("{:?}", listen_addr);
    let listen_addr = listen_addr.and_then(|l| l.parse().ok());
    let metainfo_path =
        matches.value_of("metainfo").ok_or("--seed must be set")?;
    let download_dir = matches
        .value_of("download-dir")
        .ok_or("--download-dir must be set")?;

    // optional
    let seeds: Vec<SocketAddr> = matches
        .value_of("seeds")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    println!("seeds: {:?}", seeds);

    let conf = Conf::new(download_dir);
    let (handle, mut alert_port) = cratetorrent::engine::spawn(conf)?;

    // read in torrent metainfo
    let metainfo = fs::read(metainfo_path)?;
    let metainfo = Metainfo::from_bytes(&metainfo)?;

    let mode = matches.value_of("mode").unwrap_or_default();
    let mode = match mode {
        "seed" => Mode::Seed,
        _ => Mode::Download { seeds },
    };
    println!("metainfo: {:?}", metainfo);
    println!("piece count: {}", metainfo.piece_count());
    println!("info hash: {}", hex::encode(&metainfo.info_hash));

    let _torrent_id = handle.create_torrent(TorrentParams {
        metainfo,
        listen_addr,
        mode,
        conf: None,
    })?;

    // listen to alerts from the engine
    while let Some(alert) = alert_port.next().await {
        match alert {
            Alert::TorrentStats { id, stats } => {
                println!("{}: {:#?}", id, stats);
            }
            Alert::TorrentComplete(id) => {
                println!("{} complete, shutting down", id);
                break;
            }
        }
    }

    handle.shutdown().await?;

    Ok(())
}
