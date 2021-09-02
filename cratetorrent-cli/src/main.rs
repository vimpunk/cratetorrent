use std::{io, net::SocketAddr, path::PathBuf};

use cratetorrent::prelude::*;
use flexi_logger::FileSpec;
use structopt::StructOpt;
use termion::{
    input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen,
};
use tui::{backend::TermionBackend, Terminal};

use app::App;
use key::Keys;

mod app;
mod key;
mod ui;
mod unit;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(StructOpt, Debug)]
pub struct Args {
    /// Whether to 'seed' or 'download' the torrent.
    #[structopt(
        long,
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

    #[structopt(short, long)]
    quit_after_complete: bool,
}

fn parse_mode(s: &str) -> Mode {
    match s {
        "seed" => Mode::Seed,
        _ => Mode::Download { seeds: Vec::new() },
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    flexi_logger::Logger::try_with_env()?
        .log_to_file(FileSpec::default().directory("/tmp/cratetorrent"))
        .start()?;

    // parse cli args
    let mut args = Args::from_args();
    if let Mode::Download { seeds } = &mut args.mode {
        *seeds = args.seeds.clone().unwrap_or_default();
    };

    let quit_after_complete = args.quit_after_complete;

    // set up TUI backend
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // set up app state and input events
    let mut app = App::new(args.download_dir.clone())?;
    let mut keys = Keys::new(key::EXIT_KEY);

    // for now we only support creation of a single torrent, but technically
    // everything is in place to allow running multiple torrents at the same
    // time
    app.create_torrent(args)?;

    // draw initial state
    terminal.draw(|f| ui::draw(f, &mut app))?;

    // wait for stdin input and alerts from the engine
    let mut run = true;
    while run {
        tokio::select! {
            Some(key) = keys.rx.recv() => {
                if key == key::EXIT_KEY {
                    run = false;
                }
            }
            Some(alert) = app.alert_rx.recv() => {
                match alert {
                    Alert::TorrentStats { id, stats } => {
                        app.update_torrent_state(id, *stats);
                    }
                    Alert::TorrentComplete(_) => {
                        // TODO: some notification/popup
                        if quit_after_complete {
                            run = false;
                        }
                    }
                    // TODO(https://github.com/mandreyel/cratetorrent/issues/85):
                    // handle errors
                    _ => (),
                }
            }
        }

        // draw ui with updated state
        terminal.draw(|f| ui::draw(f, &mut app))?;

        // we want to draw once more before breaking out of the loop as
        // otherwise the completion of the ui is not rendered, which will result
        // in a screen as though the app froze
        if !run {
            break;
        }
    }

    app.engine.shutdown().await?;

    Ok(())
}
