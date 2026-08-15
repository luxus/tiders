//! Rendering for the Tiders TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use tiders_core::format;
use tiders_core::model::TrackView;
use tiders_core::PlayerStatus;

use super::app::{App, Screen, View};
use super::theme;

/// Top-level draw entry point.
pub fn draw(frame: &mut Frame, app: &mut App) {
    // Paint the global background.
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::BG)),
        frame.area(),
    );

    match app.screen {
        Screen::Login => draw_login(frame, app),
        Screen::Browse => draw_browse(frame, app),
    }
}

fn draw_browse(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // tabs / search
            Constraint::Min(3),    // list
            Constraint::Length(4), // now playing
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    draw_header(frame, app, chunks[0]);
    draw_tabs(frame, app, chunks[1]);
    draw_list(frame, app, chunks[2]);
    draw_now_playing(frame, app, chunks[3]);
    draw_footer(frame, app, chunks[4]);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let mut left = vec![
        Span::styled("≈ ", Style::default().fg(theme::ACCENT2)),
        Span::styled("Tiders", theme::accent()),
        Span::styled("  TIDAL in your terminal", theme::dim()),
    ];
    if let Some(service) = app.service.as_ref() {
        if let Some(user) = service.username() {
            left.push(Span::styled("   ·   ", theme::dim()));
            left.push(Span::styled(user, Style::default().fg(theme::FG)));
        }
    }

    let right = format!(
        "{} · vol {}%",
        app.settings.quality.label(),
        app.player.volume()
    );

    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(right.len() as u16 + 1),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(Line::from(left)), header[0]);
    frame.render_widget(
        Paragraph::new(Span::styled(right, theme::dim())).alignment(Alignment::Right),
        header[1],
    );
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    if app.input_mode {
        let line = Line::from(vec![
            Span::styled(
                "  Search  ",
                Style::default().fg(theme::BG).bg(theme::ACCENT),
            ),
            Span::raw(" "),
            Span::styled(&app.input, Style::default().fg(theme::FG)),
            Span::styled("▏", Style::default().fg(theme::ACCENT)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let mut spans = Vec::new();
    for view in [View::Search, View::Favorites, View::Queue] {
        let selected = app.view == view;
        let (fg, modifier) = if selected {
            (theme::ACCENT, Modifier::BOLD)
        } else {
            (theme::DIM, Modifier::empty())
        };
        spans.push(Span::styled(
            format!("  {}  ", view.title()),
            Style::default().fg(fg).add_modifier(modifier),
        ));
        spans.push(Span::styled("│", Style::default().fg(theme::BORDER)));
    }
    spans.push(Span::styled("   press / to search", theme::dim()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let tracks = app.current_tracks();
    let title = format!(" {} ({}) ", app.view.title(), tracks.len());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .title(Span::styled(title, theme::accent()));

    if tracks.is_empty() {
        let hint = match app.view {
            View::Search => "No results yet — press / and type an artist, track, or album.",
            View::Favorites => "No favorites loaded — press f to (re)load your favorites.",
            View::Queue => "The queue is empty — pick a track and press Enter to play.",
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(theme::dim())
                .alignment(Alignment::Center)
                .block(block)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let width = area.width.saturating_sub(4) as usize;
    let now_playing_id = app.player.now_playing().map(|t| t.id);
    let items: Vec<ListItem> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| track_row(i, t, width, now_playing_id == Some(t.id)))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme::selected())
        .highlight_symbol("▍ ");

    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn track_row<'a>(
    index: usize,
    track: &'a TrackView,
    width: usize,
    is_current: bool,
) -> ListItem<'a> {
    let dur = track.duration();
    // Reserve space for index (4), duration (7), explicit badge (2), symbol (2).
    let text_budget = width.saturating_sub(4 + 7 + 3 + 2).max(8);
    let title_budget = (text_budget * 3) / 5;
    let artist_budget = text_budget.saturating_sub(title_budget);

    let marker = if is_current { "♪ " } else { "  " };
    let title = format::truncate(&track.title, title_budget);
    let artist = format::truncate(&track.artist, artist_budget.max(4));

    let mut spans = vec![
        Span::styled(marker, Style::default().fg(theme::GREEN)),
        Span::styled(format!("{:>2}. ", index + 1), theme::dim()),
        Span::styled(
            title,
            if is_current {
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::FG)
            },
        ),
        Span::styled("  ", Style::default()),
        Span::styled(artist, theme::dim()),
    ];
    if track.explicit {
        spans.push(Span::styled("  E", Style::default().fg(theme::MAGENTA)));
    }
    spans.push(Span::styled(format!("   {dur:>5}"), theme::dim()));

    ListItem::new(Line::from(spans))
}

fn draw_now_playing(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .title(Span::styled(" Now Playing ", theme::dim()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    let (icon, icon_style) = match app.player.status() {
        PlayerStatus::Playing => ("▶", Style::default().fg(theme::GREEN)),
        PlayerStatus::Paused => ("⏸", Style::default().fg(theme::YELLOW)),
        PlayerStatus::Stopped => ("⏹", Style::default().fg(theme::DIM)),
    };

    let title_line = match app.player.now_playing() {
        Some(track) => {
            let wave = spectrum(app.tick, app.player.status() == PlayerStatus::Playing);
            Line::from(vec![
                Span::styled(format!("{icon} "), icon_style),
                Span::styled(
                    track.title.clone(),
                    Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ·  ", theme::dim()),
                Span::styled(track.artist.clone(), Style::default().fg(theme::ACCENT2)),
                Span::styled("   ", theme::dim()),
                Span::styled(wave, Style::default().fg(theme::ACCENT)),
            ])
        }
        None => Line::from(vec![
            Span::styled(format!("{icon} "), icon_style),
            Span::styled("Nothing playing", theme::dim()),
        ]),
    };
    frame.render_widget(Paragraph::new(title_line), rows[0]);

    // Second row: volume gauge + queue position + backend.
    let snap = app.player.snapshot();
    let pos = match snap.queue_position {
        Some(i) if snap.queue_len > 0 => format!("{}/{}", i + 1, snap.queue_len),
        _ => "0/0".to_string(),
    };
    let meta = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(10)])
        .split(rows[1]);

    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(theme::ACCENT).bg(theme::SURFACE))
        .ratio((app.player.volume() as f64 / 100.0).clamp(0.0, 1.0))
        .label(Span::styled(
            format!("vol {}%", app.player.volume()),
            Style::default().fg(theme::BG).add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(gauge, meta[0]);

    let right = Line::from(vec![
        Span::styled("  queue ", theme::dim()),
        Span::styled(pos, Style::default().fg(theme::FG)),
        Span::styled("   backend ", theme::dim()),
        Span::styled(
            app.player.backend_name(),
            Style::default().fg(theme::ACCENT2),
        ),
    ]);
    frame.render_widget(Paragraph::new(right), meta[1]);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints: &[(&str, &str)] = if app.input_mode {
        &[("Enter", "search"), ("Esc", "cancel")]
    } else {
        &[
            ("/", "search"),
            ("Tab", "view"),
            ("↵", "play"),
            ("Spc", "pause"),
            ("n/p", "next/prev"),
            ("+/-", "vol"),
            ("f", "favorites"),
            ("q", "quit"),
        ]
    };

    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default().fg(theme::BG).bg(theme::ACCENT),
        ));
        spans.push(Span::styled(format!(" {label}   "), theme::dim()));
    }

    let status = format::truncate(&app.status, area.width as usize / 2);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(20),
            Constraint::Length(status.len() as u16 + 1),
        ])
        .split(area);
    frame.render_widget(Paragraph::new(Line::from(spans)), cols[0]);
    frame.render_widget(
        Paragraph::new(Span::styled(status, Style::default().fg(theme::YELLOW)))
            .alignment(Alignment::Right),
        cols[1],
    );
}

fn draw_login(frame: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // Header.
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("≈ ", Style::default().fg(theme::ACCENT2)),
            Span::styled("Tiders", theme::accent()),
            Span::styled("  ·  Sign in to TIDAL", theme::dim()),
        ])),
        outer[0],
    );

    let box_area = centered_rect(70, 70, outer[1]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .title(Span::styled(" Device Login ", theme::accent()));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    match app.login.as_ref() {
        Some(login) => {
            if let Some(err) = login.error.as_ref() {
                lines.push(Line::from(Span::styled(
                    "Login failed",
                    Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(err.clone(), theme::dim())));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Press r to try again, or q to quit.",
                    theme::base(),
                )));
            } else if let Some(code) = login.code.as_ref() {
                lines.push(Line::from(Span::styled(
                    "1. Open this URL in your browser:",
                    theme::base(),
                )));
                lines.push(Line::from(Span::styled(
                    format!("   {}", code.url()),
                    Style::default()
                        .fg(theme::ACCENT2)
                        .add_modifier(Modifier::UNDERLINED),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("2. Enter this code:   ", theme::base()),
                    Span::styled(
                        code.user_code.clone(),
                        Style::default()
                            .fg(theme::ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(spinner(app.tick), Style::default().fg(theme::ACCENT)),
                    Span::styled(format!(" {}", login.status), theme::dim()),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(spinner(app.tick), Style::default().fg(theme::ACCENT)),
                    Span::styled(format!(" {}", login.status), theme::dim()),
                ]));
            }
        }
        None => {
            lines.push(Line::from(Span::styled("Preparing login…", theme::dim())));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        inner,
    );

    // Footer.
    let footer = Line::from(vec![
        Span::styled(" o ", Style::default().fg(theme::BG).bg(theme::ACCENT)),
        Span::styled(" open browser   ", theme::dim()),
        Span::styled(" r ", Style::default().fg(theme::BG).bg(theme::ACCENT)),
        Span::styled(" restart   ", theme::dim()),
        Span::styled(" q ", Style::default().fg(theme::BG).bg(theme::ACCENT)),
        Span::styled(" quit", theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(footer), outer[2]);
}

/// A little braille spinner.
fn spinner(tick: u64) -> String {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(tick as usize / 2) % FRAMES.len()].to_string()
}

/// A small animated "spectrum" wave used as a nod to Maré Player's visualizer.
fn spectrum(tick: u64, animate: bool) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if !animate {
        return "▁▁▁▁▁▁▁▁".to_string();
    }
    (0..8)
        .map(|i| {
            let phase = (tick as usize + i * 3) % 14;
            let h = if phase < 8 { phase } else { 14 - phase };
            BARS[h.min(7)]
        })
        .collect()
}

/// Compute a centered rectangle occupying `pct_x`% × `pct_y`% of `area`.
fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}
