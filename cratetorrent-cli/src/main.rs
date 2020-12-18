use std::{
    collections::HashMap, fs, io, net::SocketAddr, path::PathBuf,
    time::Duration,
};

use cratetorrent::{
    prelude::*,
    storage_info::FsStructure,
    torrent::stats::{PieceStats, TorrentStats},
};
use futures::{
    select,
    stream::{Fuse, StreamExt},
};
use structopt::StructOpt;
use termion::{
    event::Key,
    input::{MouseTerminal, TermRead},
    raw::IntoRawMode,
    screen::AlternateScreen,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver},
    task,
};
use tui::{backend::TermionBackend, Terminal};

use app::App;
use key::Keys;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    // parse cli args
    let mut args = Args::from_args();
    //let seeds = args.seeds;
    if let Mode::Download { seeds } = &mut args.mode {
        *seeds = args.seeds.clone().unwrap_or_default();
    };

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
    loop {
        select! {
            key = keys.rx.select_next_some() => {
                if key == key::EXIT_KEY {
                    break;
                }
            }
            alert = app.alert_rx.select_next_some() => {
                match alert {
                    Alert::TorrentStats { id, stats } => {
                        app.update_torrent_state(id, stats);
                    }
                    Alert::TorrentComplete(_) => {
                        // TODO
                        break;
                    }
                }
            }
        }

        // update ui with state
        terminal.draw(|f| ui::draw(f, &mut app))?;
    }

    app.engine.shutdown().await?;

    Ok(())
}

mod ui {
    use std::borrow::Cow;

    use tui::{
        backend::Backend,
        layout::{Alignment, Constraint, Direction, Layout, Rect},
        style::{Color, Modifier, Style},
        text::{Span, Spans},
        widgets::{Block, Borders, Gauge, Paragraph, Sparkline},
        Frame,
    };

    use crate::app::{App, ThroughputHistory, Torrent};

    pub fn draw(f: &mut Frame<impl Backend>, app: &mut App) {
        // TODO: For now the app only supports a single torrent. Later we'll
        // want to display a list of torrents and what were rendering below
        // will become a separate tab.

        if let Some(torrent) = app.torrents.values().next() {
            // split window into 3 horizontal chunks:
            // - first is split vertically between torrent info and throughput rates
            // - second is a thin slice for the progress bar
            // - third is the remaining (large) area split vertically between files
            //   and later peers info
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Percentage(30),
                        Constraint::Percentage(5),
                        Constraint::Percentage(65),
                    ]
                    .as_ref(),
                )
                .split(f.size());
            draw_info(f, torrent, chunks[0]);
            draw_progress_bar(f, torrent, chunks[1]);
            draw_details(f, torrent, chunks[1]);
        }
    }

    pub fn draw_info(
        f: &mut Frame<impl Backend>,
        torrent: &Torrent,
        area: Rect,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(
                [Constraint::Percentage(30), Constraint::Percentage(70)]
                    .as_ref(),
            )
            .split(area);
        draw_metadata(f, torrent, chunks[0]);
        draw_throughput(f, torrent, chunks[1]);
    }

    pub fn draw_metadata(
        f: &mut Frame<impl Backend>,
        torrent: &Torrent,
        area: Rect,
    ) {
        let text = vec![
            // TODO: avoid clones here
            create_key_value_spans("Name: ", torrent.name.clone()),
            create_key_value_spans("Info hash: ", torrent.info_hash.clone()),
            create_key_value_spans(
                "Piece len: ",
                torrent.piece_len.to_string(),
            ),
            create_key_value_spans(
                "Elapsed: ",
                format!("{} s", torrent.run_duration.as_secs()),
            ),
            create_key_value_spans(
                "Pieces: ",
                format!(
                    "{}/{} (pending: {})",
                    torrent.pieces.complete,
                    torrent.pieces.total,
                    torrent.pieces.pending
                ),
            ),
        ];

        let paragraph = Paragraph::new(text.clone())
            .block(create_block("Metadata"))
            .alignment(Alignment::Left);
        f.render_widget(paragraph, area);
    }

    pub fn draw_throughput(
        f: &mut Frame<impl Backend>,
        torrent: &Torrent,
        area: Rect,
    ) {
        // we're showing 4 throughput stats:
        // - download rate
        // - upload rate
        // - received protcol chatter rate
        // - sent protcol chatter rate
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Percentage(35),
                    Constraint::Percentage(35),
                    Constraint::Percentage(15),
                    Constraint::Percentage(15),
                ]
                .as_ref(),
            )
            .split(area);

        let mut draw_rate_sparkline =
            |title: &'static str, stats: &ThroughputHistory, chunk| {
                let title = if let Some(rate) =
                    stats.rate_history.iter().rev().next()
                {
                    create_key_value_spans(
                        title,
                        format!("{} b/s (peak: {})", rate, stats.peak),
                    )
                } else {
                    create_key_value_spans(title, "n/a".to_string())
                };
                let sparkline = Sparkline::default()
                    .block(create_block_with_spans(title))
                    .data(&stats.rate_history)
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(sparkline, chunk);
            };

        draw_rate_sparkline(
            "Download: ",
            &torrent.downloaded_payload_stats,
            chunks[0],
        );
        draw_rate_sparkline(
            "Upload: ",
            &torrent.uploaded_payload_stats,
            chunks[1],
        );
        draw_rate_sparkline(
            "Recv prot: ",
            &torrent.downloaded_protocol_stats,
            chunks[2],
        );
        draw_rate_sparkline(
            "Sent prot: ",
            &torrent.uploaded_protocol_stats,
            chunks[3],
        );
    }

    pub fn draw_progress_bar(
        f: &mut Frame<impl Backend>,
        torrent: &Torrent,
        area: Rect,
    ) {
        let downloaded_percent = (torrent.pieces.complete as f64
            / torrent.pieces.total as f64)
            * 100.0;
        let progress = Gauge::default()
            .block(create_block("Completion"))
            .gauge_style(Style::default().fg(Color::Yellow))
            .percent(downloaded_percent as u16);
        f.render_widget(progress, area);
    }

    pub fn draw_details(
        f: &mut Frame<impl Backend>,
        torrent: &Torrent,
        area: Rect,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(
                [Constraint::Percentage(50), Constraint::Percentage(50)]
                    .as_ref(),
            )
            .split(area);
        // TODO
    }

    fn create_block<'a>(title: impl Into<Cow<'a, str>>) -> Block<'a> {
        let title =
            Span::styled(title, Style::default().add_modifier(Modifier::BOLD));
        create_block_with_spans(title)
    }

    fn create_block_with_spans<'a>(title: impl Into<Spans<'a>>) -> Block<'a> {
        Block::default().borders(Borders::ALL).title(title)
    }

    fn create_key_value_spans<'a>(
        title: impl Into<Cow<'a, str>>,
        value: impl Into<Cow<'a, str>>,
    ) -> Spans<'a> {
        Spans(vec![
            Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                value,
                Style::default().add_modifier(Modifier::ITALIC),
            ),
        ])
    }
}

mod app {
    use cratetorrent::torrent::stats::ThroughputStats;

    use super::*;

    /// Holds the application state.
    pub struct App {
        pub engine: EngineHandle,
        pub alert_rx: Fuse<AlertReceiver>,
        pub torrents: HashMap<TorrentId, Torrent>,
    }

    /// Holds state about a single torrent.
    pub struct Torrent {
        // static info
        pub name: String,
        pub info_hash: String,
        pub piece_len: u32,
        pub fs: FsStructure,
        // TODO
        // total download size

        // dynamic info
        pub run_duration: Duration,
        pub pieces: PieceStats,
        pub peer_count: usize,

        pub downloaded_payload_stats: ThroughputHistory,
        pub wasted_payload_count: u64,
        pub uploaded_payload_stats: ThroughputHistory,
        pub downloaded_protocol_stats: ThroughputHistory,
        pub uploaded_protocol_stats: ThroughputHistory,
        //
        // TODO:
        // downloaded
        // list of files with their sizes (map fs to vec<files>)
    }

    impl App {
        pub fn new(download_dir: PathBuf) -> Result<Self> {
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

        pub fn create_torrent(&mut self, args: Args) -> Result<()> {
            // read in torrent metainfo
            let metainfo = fs::read(&args.metainfo)?;
            let metainfo = Metainfo::from_bytes(&metainfo)?;
            let info_hash = hex::encode(&metainfo.info_hash);
            let piece_count = metainfo.piece_count();

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
                piece_len: metainfo.piece_len,
                fs: metainfo.structure,

                run_duration: Default::default(),
                pieces: PieceStats {
                    total: piece_count,
                    ..Default::default()
                },
                peer_count: Default::default(),
                downloaded_payload_stats: Default::default(),
                wasted_payload_count: Default::default(),
                uploaded_payload_stats: Default::default(),
                downloaded_protocol_stats: Default::default(),
                uploaded_protocol_stats: Default::default(),
            };
            self.torrents.insert(torrent_id, torrent);

            Ok(())
        }

        pub fn update_torrent_state(
            &mut self,
            torrent_id: TorrentId,
            stats: TorrentStats,
        ) {
            if let Some(torrent) = self.torrents.get_mut(&torrent_id) {
                torrent.run_duration = stats.run_duration;
                torrent.pieces = stats.pieces;
                torrent.peer_count = stats.peer_count;

                const HISTORY_LIMIT: usize = 100;
                for (history, update) in [
                    (
                        &mut torrent.downloaded_payload_stats,
                        &stats.downloaded_payload_stats,
                    ),
                    (
                        &mut torrent.uploaded_payload_stats,
                        &stats.uploaded_payload_stats,
                    ),
                    (
                        &mut torrent.downloaded_protocol_stats,
                        &stats.downloaded_protocol_stats,
                    ),
                    (
                        &mut torrent.uploaded_protocol_stats,
                        &stats.uploaded_protocol_stats,
                    ),
                ]
                .iter_mut()
                {
                    history.update(update, HISTORY_LIMIT);
                }
            }
        }
    }

    #[derive(Default)]
    pub struct ThroughputHistory {
        pub peak: u64,
        pub total: u64,
        pub rate_history: Vec<u64>,
    }

    impl ThroughputHistory {
        pub fn update(&mut self, stats: &ThroughputStats, limit: usize) {
            if self.rate_history.len() >= limit {
                // pop the first element
                // TODO: make this more optimal
                self.rate_history.drain(0..1);
            }
            self.rate_history.push(stats.rate);
            self.peak = stats.peak;
            self.total = stats.total;
        }
    }
}

mod key {
    use super::*;

    /// A small event handler that reads termion input events on a bocking tokio
    /// task and forwards them via an mpsc channel to the main task. This can be
    /// used to drive the application.
    pub struct Keys {
        /// Input keys are sent to this channel.
        pub rx: Fuse<UnboundedReceiver<Key>>,
    }

    impl Keys {
        pub fn new(exit_key: Key) -> Self {
            let (tx, rx) = mpsc::unbounded_channel();
            task::spawn(async move {
                let stdin = io::stdin();
                for key in stdin.keys() {
                    if let Ok(key) = key {
                        if let Err(err) = tx.send(key) {
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

    pub const EXIT_KEY: Key = Key::Char('q');
}

fn parse_mode(s: &str) -> Mode {
    match s {
        "seed" => Mode::Seed,
        _ => Mode::Download { seeds: Vec::new() },
    }
}
