use std::borrow::Cow;

use cratetorrent::{peer::ConnectionState, torrent::stats::Peers};
use tui::{
    backend::Backend,
    layout::{Alignment, Constraint, Corner, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Sparkline},
    Frame,
};

use crate::{
    app::{App, ThruputHistory, Torrent},
    unit::Unit,
};

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
                    Constraint::Percentage(27),
                    Constraint::Percentage(8),
                    Constraint::Percentage(65),
                ]
                .as_ref(),
            )
            .split(f.size());

        {
            let area = chunks[0];
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(
                    [Constraint::Percentage(40), Constraint::Percentage(60)]
                        .as_ref(),
                )
                .split(area);
            draw_metadata(f, torrent, chunks[0]);
            draw_throughput(f, torrent, chunks[1]);
        }

        draw_progress_bar(f, torrent, chunks[1]);

        {
            let area = chunks[2];
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(
                    [
                        Constraint::Percentage(43),
                        Constraint::Percentage(7),
                        Constraint::Percentage(50),
                    ]
                    .as_ref(),
                )
                .split(area);
            draw_files(f, torrent, chunks[0]);
            draw_pieces(f, torrent, chunks[1]);
            draw_peers(f, torrent, chunks[2]);
        }
    }
}

pub fn draw_metadata(
    f: &mut Frame<impl Backend>,
    torrent: &Torrent,
    area: Rect,
) {
    let text = vec![
        // TODO: avoid clones here
        create_key_value_spans("Name: ", torrent.name.clone()),
        create_key_value_spans(
            "Path: ",
            torrent
                .storage
                .download_dir
                .join(&torrent.name)
                .display()
                .to_string(),
        ),
        create_key_value_spans("Info hash: ", torrent.info_hash.clone()),
        create_key_value_spans(
            "Size: ",
            Unit::new(torrent.storage.download_len).to_string(),
        ),
        create_key_value_spans(
            "Piece len: ",
            Unit::new(torrent.piece_len as u64).to_string(),
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
        create_key_value_spans(
            "Connected peers: ",
            torrent.peers.len().to_string(),
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
        |title: &'static str, stats: &ThruputHistory, chunk| {
            let title = create_key_value_spans(
                title,
                format!(
                    "{}/s (peak: {})",
                    Unit::new(stats.rate()),
                    Unit::new(stats.peak)
                ),
            );
            let sparkline = Sparkline::default()
                .block(create_block_with_spans(title))
                .data(&stats.rates)
                .style(Style::default().fg(Color::Yellow));
            f.render_widget(sparkline, chunk);
        };

    draw_rate_sparkline("Down: ", &torrent.payload.down, chunks[0]);
    draw_rate_sparkline("Up: ", &torrent.payload.up, chunks[1]);
    draw_rate_sparkline("Protocol down: ", &torrent.protocol.down, chunks[2]);
    draw_rate_sparkline("Protocol up: ", &torrent.protocol.up, chunks[3]);
}

pub fn draw_progress_bar(
    f: &mut Frame<impl Backend>,
    torrent: &Torrent,
    area: Rect,
) {
    let downloaded_percent =
        (torrent.pieces.complete as f64 / torrent.pieces.total as f64) * 100.0;
    let progress = Gauge::default()
        .block(create_block("Completion"))
        .gauge_style(Style::default().fg(Color::Yellow))
        .percent(downloaded_percent as u16);
    f.render_widget(progress, area);
}

pub fn draw_files(f: &mut Frame<impl Backend>, torrent: &Torrent, area: Rect) {
    let files: Vec<ListItem> = torrent
        .files
        .iter()
        .map(|file| {
            let delim = Spans::from("-".repeat(area.width as usize));
            let path = Spans::from(Span::styled(
                file.info.path.display().to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            let downloaded_percent =
                (file.complete as f64 / file.info.len as f64) * 100.0;
            let file_size = Spans::from(format!(
                "{}/{} ({}%)",
                Unit::new(file.complete),
                Unit::new(file.info.len),
                downloaded_percent as u16
            ));
            // Build up the final list item:
            // 1. Add a `---` spacing line above the list entry
            // 2. Add the file name/path as bold
            // 3. Add a spacer line
            // 4. Add the file size
            ListItem::new(vec![delim, path, Spans::from(""), file_size])
        })
        .collect();
    let files = List::new(files).block(create_block("Files"));
    f.render_widget(files, area);
}

pub fn draw_pieces(f: &mut Frame<impl Backend>, torrent: &Torrent, area: Rect) {
    let pieces: Vec<ListItem> = torrent
        .pieces
        .latest_completed
        .as_ref()
        .map(|latest_completed| {
            latest_completed
                .iter()
                .map(|piece| {
                    let delim = Spans::from("-".repeat(area.width as usize));
                    let piece_index = Spans::from(Span::styled(
                        piece.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    // Build up the final list item:
                    // 1. Add a `---` spacing line above the list entry
                    // 2. Add piece index
                    ListItem::new(vec![delim, piece_index])
                })
                .collect()
        })
        .unwrap_or_default();
    let pieces = List::new(pieces)
        .block(create_block("Pieces"))
        .start_corner(Corner::BottomLeft);
    f.render_widget(pieces, area);
}

pub fn draw_peers(f: &mut Frame<impl Backend>, torrent: &Torrent, area: Rect) {
    if let Peers::Full(peers) = &torrent.peers {
        let peers: Vec<ListItem> = peers
            .iter()
            .map(|peer| {
                let delim = Spans::from("-".repeat(area.width as usize));

                let header = {
                    // always show the peer's address
                    let mut buf = peer.addr.to_string();

                    // peer id: we may not be able to show it if we don't have
                    // it yet because peer is still connecting or if it's not
                    // valid utf8
                    if let Some(id) = peer
                        .id
                        .and_then(|id| String::from_utf8(id.to_vec()).ok())
                    {
                        buf += " :: ";
                        buf += &id;
                    } else if peer.state.connection
                        == ConnectionState::Connecting
                    {
                        buf += " :: connecting...";
                    }

                    // peer seed/leech state
                    if peer.piece_count == torrent.pieces.total {
                        buf += " :: seed";
                    } else {
                        let downloaded_percent = (peer.piece_count as f64
                            / torrent.pieces.total as f64)
                            * 100.0;
                        let downloaded_percent = downloaded_percent as u16;
                        buf += " :: ";
                        buf += &downloaded_percent.to_string();
                        buf += "%";
                    }

                    // peer session state
                    let mut state = String::with_capacity(4);
                    if peer.state.is_choked {
                        state += "C";
                    }
                    if peer.state.is_interested {
                        state += "I";
                    }
                    if peer.state.is_peer_choked {
                        state += "c";
                    }
                    if peer.state.is_peer_interested {
                        state += "i";
                    }
                    if !state.is_empty() {
                        buf += " :: ";
                        buf += &state;
                    }

                    buf
                };
                let peer_header = Spans::from(Span::styled(
                    header,
                    Style::default().add_modifier(Modifier::BOLD),
                ));

                let stats = Spans(vec![Span::from(format!(
                    "down: {}/s (peak: {}, wasted: {}) \
                    - up: {}/s (peak: {})",
                    Unit::new(peer.thruput.payload.down.rate),
                    Unit::new(peer.thruput.payload.down.peak),
                    Unit::new(peer.thruput.waste),
                    Unit::new(peer.thruput.payload.up.rate),
                    Unit::new(peer.thruput.payload.up.peak),
                ))]);
                // Build up the final list item:
                // 1. Add a `---` spacing line above the list entry
                // 2. Add peer address and optionally client id
                // 3. Add a spacer line
                // 4. Add stats about the peer
                ListItem::new(vec![delim, peer_header, Spans::from(""), stats])
            })
            .collect();
        let peers = List::new(peers).block(create_block("Peers"));
        f.render_widget(peers, area);
    }
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
        Span::styled(value, Style::default().add_modifier(Modifier::ITALIC)),
    ])
}
