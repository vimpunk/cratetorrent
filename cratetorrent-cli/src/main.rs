use std::{fs, net::SocketAddr, path::PathBuf};

use cratetorrent::prelude::*;
use futures::stream::StreamExt;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Args {
    /// Whether to 'seed' or 'download' the torrent.
    #[structopt(parse(from_str = parse_mode))]
    mode: Mode,

    /// The path of the folder where to download file.
    download_dir: PathBuf,

    /// The path to the torrent metainfo file.
    metainfo: PathBuf,

    /// A comma separated list of <ip>:<port> pairs of the seeds.
    #[structopt(default_value = "Vec::new()")]
    seeds: Vec<SocketAddr>,

    /// The socket address on which to listen for new connections.
    listen: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // parse cli args
    let mut args = Args::from_args();
    println!("{:#?}", args);
    //let seeds = args.seeds;
    if let Mode::Download { seeds } = &mut args.mode {
        *seeds = args.seeds;
    };

    // read in torrent metainfo
    let metainfo = fs::read(&args.metainfo)?;
    let metainfo = Metainfo::from_bytes(&metainfo)?;
    println!("metainfo: {:?}", metainfo);
    println!("piece count: {}", metainfo.piece_count());
    println!("info hash: {}", hex::encode(&metainfo.info_hash));

    // start engine
    let conf = Conf::new(args.download_dir);
    let (handle, mut alert_rx) = cratetorrent::engine::spawn(conf)?;

    // create torrent
    let _torrent_id = handle.create_torrent(TorrentParams {
        metainfo,
        listen_addr: args.listen,
        mode: args.mode,
        conf: None,
    })?;

    // listen to alerts from the engine
    while let Some(alert) = alert_rx.next().await {
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

fn parse_mode(s: &str) -> Mode {
    match s {
        "seed" => Mode::Seed,
        _ => Mode::Download { seeds: Vec::new() },
    }
}
