use std::{path::PathBuf, net::SocketAddr};

use cratetorrent::prelude::*;
use futures::stream::StreamExt;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct Args {
    /// Whether to 'seed' or 'download' the torrent.
    #[structopt(
        short, long,
        parse(from_str = parse_mode),
        default_value = "Mode::Download { seeds: Vec::new() }",
    )]
    mode: Mode,

    /// The path of the folder where to download file.
    #[structopt(short, long)]
    download_dir: PathBuf,

    /// The path to the torrent metainfo file.
    #[structopt(short, long)]
    metainfo: PathBuf,

    /// A comma separated list of <ip>:<port> pairs of the seeds.
    #[structopt(short, long)]
    seeds: Option<Vec<SocketAddr>>,

    /// The socket address on which to listen for new connections.
    #[structopt(short, long)]
    listen: Option<SocketAddr>,
}

fn parse_mode(s: &str) -> Mode {
    match s {
        "seed" => Mode::Seed,
        _ => Mode::Download { seeds: Vec::new() },
    }
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // parse cli args
    let mut args = Args::from_args();
    if let Mode::Download { seeds } = &mut args.mode {
        *seeds = args.seeds.clone().unwrap_or_default();
    };

    // spawn the torrent engine
    let conf = Conf::new(args.download_dir);
    let (handle, mut alert_rx) = cratetorrent::engine::spawn(conf)?;

    // read in torrent metainfo
    let metainfo = tokio::fs::read(&args.metainfo).await?;
    let metainfo = Metainfo::from_bytes(&metainfo)?;

    println!("metainfo: {:?}", metainfo);
    println!("piece count: {}", metainfo.piece_count());
    println!("info hash: {}", hex::encode(&metainfo.info_hash));

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
            // we don't care about other errors here
            _ => (),
        }
    }

    handle.shutdown().await?;

    Ok(())
}
