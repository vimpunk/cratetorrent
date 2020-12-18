use std::{
    collections::HashMap,
    fs, io,
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use cratetorrent::{prelude::*, storage_info::FsStructure};
use futures::{
    select,
    stream::{Fuse, StreamExt},
};
use structopt::StructOpt;
use termion::event::Key;
use termion::input::TermRead;
use tokio::{
    sync::mpsc::{self, UnboundedReceiver},
    task,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(StructOpt, Debug)]
struct Args {
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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    // parse cli args
    let mut args = Args::from_args();
    println!("{:#?}", args);
    //let seeds = args.seeds;
    if let Mode::Download { seeds } = &mut args.mode {
        *seeds = args.seeds.clone().unwrap_or_default();
    };

    let mut app = App::new(args.download_dir.clone())?;
    let mut keys = Keys::new(EXIT_KEY);

    app.create_torrent(args)?;

    // wait for stdin input and alerts from the engine
    loop {
        select! {
            key = keys.rx.select_next_some() => {
                if key == EXIT_KEY {
                    break;
                }
            }
            alert = app.alert_rx.select_next_some() => {
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
        }
    }

    app.engine.shutdown().await?;

    Ok(())
}

/// Holds the application state.
struct App {
    engine: EngineHandle,
    alert_rx: Fuse<AlertReceiver>,
    torrents: HashMap<TorrentId, Torrent>,
}

/// Holds state about a single torrent.
struct Torrent {
    name: String,
    info_hash: String,
    start_time: SystemTime,
    run_duration: Duration,
    piece_count: usize,
    piece_len: u32,
    fs: FsStructure,
}

impl App {
    fn new(download_dir: PathBuf) -> Result<Self> {
        // start engine
        let conf = Conf::new(download_dir);
        let (engine, alert_rx) = cratetorrent::engine::spawn(conf)?;
        let alert_rx = alert_rx.fuse();

        Ok(Self {
            engine,
            alert_rx,
            torrents: HashMap::new(),
        })
    }

    fn create_torrent(&mut self, args: Args) -> Result<()> {
        // read in torrent metainfo
        let metainfo = fs::read(&args.metainfo)?;
        let metainfo = Metainfo::from_bytes(&metainfo)?;
        let info_hash = hex::encode(&metainfo.info_hash);
        let piece_count = metainfo.piece_count();
        println!("metainfo: {:?}", metainfo);
        println!("piece count: {}", piece_count);
        println!("info hash: {}", info_hash);

        // create torrent
        let torrent_id = self.engine.create_torrent(TorrentParams {
            metainfo: metainfo.clone(),
            listen_addr: args.listen,
            mode: args.mode,
            conf: None,
        })?;

        let torrent = Torrent {
            name: metainfo.name,
            info_hash,
            start_time: SystemTime::now(),
            run_duration: Default::default(),
            piece_count,
            piece_len: metainfo.piece_len,
            fs: metainfo.structure,
        };
        self.torrents.insert(torrent_id, torrent);

        Ok(())
    }
}

/// A small event handler that reads termion input events on a bocking tokio
/// task and forwards them via an mpsc channel to the main task. This can be
/// used to drive the application.
struct Keys {
    /// Input keys are sent to this channel.
    rx: Fuse<UnboundedReceiver<Key>>,
}

impl Keys {
    fn new(exit_key: Key) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        task::spawn(async move {
            let stdin = io::stdin();
            for key in stdin.keys() {
                if let Ok(key) = key {
                    if let Err(err) = tx.send(key) {
                        eprintln!("{}", err);
                        return;
                    }
                    if key == exit_key {
                        return;
                    }
                }
            }
        });
        Self { rx: rx.fuse() }
    }
}

const EXIT_KEY: Key = Key::Char('q');

fn parse_mode(s: &str) -> Mode {
    match s {
        "seed" => Mode::Seed,
        _ => Mode::Download { seeds: Vec::new() },
    }
}
