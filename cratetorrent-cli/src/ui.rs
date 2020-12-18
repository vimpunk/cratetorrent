
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

pub fn draw_info(f: &mut Frame<impl Backend>, torrent: &Torrent, area: Rect) {
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
        create_key_value_spans("Info hash: ", torrent.info_hash.clone()),
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

pub fn draw_details(
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
        Span::styled(value, Style::default().add_modifier(Modifier::ITALIC)),
    ])
}
