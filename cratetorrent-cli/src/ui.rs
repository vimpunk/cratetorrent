use std::borrow::Cow;

use tui::{
    backend::Backend,
    layout::{Alignment, Constraint, Corner, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Sparkline},
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
        draw_metadata_and_throughput(f, torrent, chunks[0]);
        draw_progress_bar(f, torrent, chunks[1]);
        draw_files_and_pieces(f, torrent, chunks[2]);
    }
}

pub fn draw_metadata_and_throughput(
    f: &mut Frame<impl Backend>,
    torrent: &Torrent,
    area: Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [Constraint::Percentage(30), Constraint::Percentage(70)].as_ref(),
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
            format!("{} b", torrent.storage.download_len),
        ),
        create_key_value_spans("Piece len: ", torrent.piece_len.to_string()),
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
            torrent.peer_count.to_string(),
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

    let mut draw_rate_sparkline = |title: &'static str,
                                   stats: &ThroughputHistory,
                                   chunk| {
        let title = if let Some(rate) = stats.rate_history.iter().rev().next() {
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
    draw_rate_sparkline("Upload: ", &torrent.uploaded_payload_stats, chunks[1]);
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
    let downloaded_percent =
        (torrent.pieces.complete as f64 / torrent.pieces.total as f64) * 100.0;
    let progress = Gauge::default()
        .block(create_block("Completion"))
        .gauge_style(Style::default().fg(Color::Yellow))
        .percent(downloaded_percent as u16);
    f.render_widget(progress, area);
}

pub fn draw_files_and_pieces(
    f: &mut Frame<impl Backend>,
    torrent: &Torrent,
    area: Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [Constraint::Percentage(50), Constraint::Percentage(50)].as_ref(),
        )
        .split(area);
    draw_files(f, torrent, chunks[0]);
    draw_pieces(f, torrent, chunks[1]);
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
                "{}/{} bytes ({}%)",
                file.complete, file.info.len, downloaded_percent as u16
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
        .block(create_block("Latest pieces"))
        .start_corner(Corner::BottomLeft);
    f.render_widget(pieces, area);
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
