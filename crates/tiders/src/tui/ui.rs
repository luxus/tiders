//! Rendering for the Tiders TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::{Resize, StatefulImage};

use tiders_core::format;
use tiders_core::model::TrackView;
use tiders_core::PlayerStatus;

use super::app::{App, Popup, Screen, View, POPUP_OPEN_FRAMES, QUALITIES};
use super::theme;

/// Top-level draw entry point.
pub fn draw(frame: &mut Frame, app: &mut App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::BG)),
        frame.area(),
    );

    match app.screen {
        Screen::Login => draw_login(frame, app),
        Screen::Browse => draw_browse(frame, app),
    }

    if app.popup.is_some() {
        draw_popup(frame, app);
    }
    draw_toast(frame, app);
}

fn draw_browse(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // tabs / search
            Constraint::Min(3),    // list
            Constraint::Length(6), // now playing (with art)
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
    // Gradient wordmark.
    let mut left = gradient_spans("≈ Tiders", theme::ACCENT2, theme::ACCENT);
    left.push(Span::styled("  TIDAL in your terminal", theme::dim()));
    if let Some(service) = app.service.as_ref() {
        if let Some(user) = service.username() {
            left.push(Span::styled("   ·   ", theme::dim()));
            left.push(Span::styled(user, Style::default().fg(theme::FG)));
        }
    }
    if app.loading {
        left.push(Span::styled(
            format!("   {} loading…", spinner(app.tick)),
            Style::default().fg(theme::ACCENT2),
        ));
    }

    let right = format!(
        "{} · vol {}% · img:{}",
        app.settings.quality.label(),
        app.player.volume(),
        app.art.protocol_label()
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(right.len() as u16 + 1),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(Line::from(left)), cols[0]);
    frame.render_widget(
        Paragraph::new(Span::styled(right, theme::dim())).alignment(Alignment::Right),
        cols[1],
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
            Span::styled(
                if app.tick % 8 < 4 { "▏" } else { " " },
                Style::default().fg(theme::ACCENT),
            ),
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
    spans.push(Span::styled(
        "   / search   d details   Q quality   ? help",
        theme::dim(),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let tracks = app.current_tracks();
    let spin = if app.loading {
        format!(" {} ", spinner(app.tick))
    } else {
        String::new()
    };
    let title = format!(" {} ({}){}", app.view.title(), tracks.len(), spin);

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

fn draw_now_playing(frame: &mut Frame, app: &mut App, area: Rect) {
    // Gather everything we need before mutably borrowing the art protocol.
    let status = app.player.status();
    let np = app.player.now_playing().cloned();
    let volume = app.player.volume();
    let snap = app.player.snapshot();
    let backend = app.player.backend_name().to_string();
    let elapsed = app.elapsed_secs();
    let progress = app.progress();
    let tick = app.tick;
    let has_art = app.now_art.is_some();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .title(Span::styled(" Now Playing ", theme::dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split off a square-ish album-art column on the left when we have art.
    let art_w = if has_art { inner.height * 2 } else { 0 };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_w), Constraint::Min(10)])
        .split(inner);
    let art_area = cols[0];
    let info_area = cols[1];

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // spectrum
            Constraint::Length(1), // progress
            Constraint::Length(1), // meta
        ])
        .split(info_area);

    let (icon, icon_style) = match status {
        PlayerStatus::Playing => ("▶", Style::default().fg(theme::GREEN)),
        PlayerStatus::Paused => ("⏸", Style::default().fg(theme::YELLOW)),
        PlayerStatus::Stopped => ("⏹", Style::default().fg(theme::DIM)),
    };

    match &np {
        Some(track) => {
            let title_w = rows[0].width.saturating_sub(3) as usize;
            let title = marquee(&track.title, title_w, tick, status == PlayerStatus::Playing);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{icon} "), icon_style),
                    Span::styled(
                        title,
                        Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ·  {}", track.artist),
                        Style::default().fg(theme::ACCENT2),
                    ),
                ])),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new(spectrum_line(tick, status == PlayerStatus::Playing, 24)),
                rows[1],
            );

            // Progress bar with elapsed / total.
            let total = track.duration_secs;
            let bar_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(6),
                    Constraint::Min(6),
                    Constraint::Length(6),
                ])
                .split(rows[2]);
            frame.render_widget(
                Paragraph::new(Span::styled(format::duration(elapsed), theme::dim())),
                bar_cols[0],
            );
            frame.render_widget(
                Gauge::default()
                    .gauge_style(Style::default().fg(theme::ACCENT).bg(theme::SURFACE))
                    .ratio(progress)
                    .label(""),
                bar_cols[1],
            );
            frame.render_widget(
                Paragraph::new(Span::styled(format::duration(total), theme::dim()))
                    .alignment(Alignment::Right),
                bar_cols[2],
            );
        }
        None => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{icon} "), icon_style),
                    Span::styled("Nothing playing", theme::dim()),
                ])),
                rows[0],
            );
        }
    }

    // Meta row: volume gauge + queue + backend.
    let pos = match snap.queue_position {
        Some(i) if snap.queue_len > 0 => format!("{}/{}", i + 1, snap.queue_len),
        _ => "0/0".to_string(),
    };
    let meta = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(16), Constraint::Min(10)])
        .split(rows[3]);
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(theme::ACCENT2).bg(theme::SURFACE))
            .ratio((volume as f64 / 100.0).clamp(0.0, 1.0))
            .label(Span::styled(
                format!("vol {volume}%"),
                Style::default().fg(theme::BG).add_modifier(Modifier::BOLD),
            )),
        meta[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  queue ", theme::dim()),
            Span::styled(pos, Style::default().fg(theme::FG)),
            Span::styled("   backend ", theme::dim()),
            Span::styled(backend, Style::default().fg(theme::ACCENT2)),
        ])),
        meta[1],
    );

    // Finally, the album art (mutably borrows the protocol).
    if art_area.width > 0 {
        if let Some(art) = app.now_art.as_mut() {
            frame.render_stateful_widget(
                StatefulImage::default().resize(Resize::Fit(None)),
                art_area,
                art,
            );
        }
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints: &[(&str, &str)] = if app.input_mode {
        &[("Enter", "search"), ("Esc", "cancel")]
    } else {
        &[
            ("/", "search"),
            ("↵", "play"),
            ("Spc", "pause"),
            ("n/p", "next/prev"),
            ("d", "details"),
            ("Q", "quality"),
            ("?", "help"),
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
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── popups ──────────────────────────────────────────────────────────────

fn draw_popup(frame: &mut Frame, app: &mut App) {
    let factor = (app.popup_anim as f32 / POPUP_OPEN_FRAMES as f32).clamp(0.2, 1.0);
    let (base_w, base_h) = match app.popup {
        Some(Popup::Help) => (56u16, 74u16),
        Some(Popup::Detail(_)) => (70, 78),
        Some(Popup::Quality) => (38, 48),
        None => return,
    };
    let pct_w = ((base_w as f32) * factor) as u16;
    let pct_h = ((base_h as f32) * factor) as u16;
    let area = centered_pct(pct_w.max(18), pct_h.max(18), frame.area());

    frame.render_widget(Clear, area);

    // Extract the popup kind (owning any needed data) so the app can be mutably
    // borrowed again below for image rendering.
    enum Kind {
        Help,
        Quality,
        Detail(Box<TrackView>),
    }
    let kind = match &app.popup {
        Some(Popup::Help) => Kind::Help,
        Some(Popup::Quality) => Kind::Quality,
        Some(Popup::Detail(track)) => Kind::Detail(Box::new(track.clone())),
        None => return,
    };
    let title = match &kind {
        Kind::Help => " Help ",
        Kind::Detail(_) => " Track Details ",
        Kind::Quality => " Audio Quality ",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .style(Style::default().bg(theme::SURFACE))
        .title(Span::styled(title, theme::accent()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // While the box is still expanding, show just the frame (nice "open" feel).
    if factor < 0.85 {
        return;
    }

    match kind {
        Kind::Help => draw_help(frame, inner),
        Kind::Quality => draw_quality(frame, app.quality_cursor, inner),
        Kind::Detail(track) => draw_detail(frame, app, &track, inner),
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let key = |k: &str| {
        Span::styled(
            format!(" {k} "),
            Style::default().fg(theme::BG).bg(theme::ACCENT),
        )
    };
    let desc = |d: &str| Span::styled(format!("  {d}"), theme::base());
    let rows = [
        ("/", "search the catalog"),
        ("Tab / 1 2 3", "switch Search · Favorites · Queue"),
        ("↑ ↓ / k j", "move selection"),
        ("Enter", "play the highlighted track"),
        ("Space", "play / pause"),
        ("n / p", "next / previous track"),
        ("+ / -", "volume up / down"),
        ("s", "stop"),
        ("d", "track details (with cover art)"),
        ("Q", "change audio quality"),
        ("f", "reload favorites"),
        ("? / Esc", "toggle this help / close"),
        ("q", "quit"),
    ];
    let mut lines = vec![Line::from("")];
    for (k, d) in rows {
        lines.push(Line::from(vec![key(k), desc(d)]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tiders — TIDAL in your terminal",
        theme::dim(),
    )));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        inset(area, 2, 1),
    );
}

fn draw_quality(frame: &mut Frame, cursor: usize, area: Rect) {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Select streaming quality:", theme::dim())),
        Line::from(""),
    ];
    for (i, q) in QUALITIES.iter().enumerate() {
        let selected = i == cursor;
        let marker = if selected { "►" } else { " " };
        let style = if selected {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::base()
        };
        lines.push(Line::from(Span::styled(
            format!("   {marker} {}", q.label()),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ choose · Enter apply · Esc cancel",
        theme::dim(),
    )));
    frame.render_widget(Paragraph::new(lines), inset(area, 2, 1));
}

fn draw_detail(frame: &mut Frame, app: &mut App, track: &TrackView, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(inset(area, 1, 1));

    // Metadata on the right.
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            track.title.clone(),
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            track.artist.clone(),
            Style::default().fg(theme::ACCENT2),
        )),
        Line::from(""),
    ];
    if let Some(album) = &track.album {
        lines.push(Line::from(vec![
            Span::styled("Album   ", theme::dim()),
            Span::styled(album.clone(), theme::base()),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Length  ", theme::dim()),
        Span::styled(track.duration(), theme::base()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Track   ", theme::dim()),
        Span::styled(format!("#{}", track.id), theme::base()),
    ]));
    if track.explicit {
        lines.push(Line::from(Span::styled(
            "Explicit",
            Style::default().fg(theme::MAGENTA),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter / Esc to close · Enter on the list to play",
        theme::dim(),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), cols[1]);

    // Cover art on the left.
    if let Some(art) = app.detail_art.as_mut() {
        frame.render_stateful_widget(
            StatefulImage::default().resize(Resize::Fit(None)),
            cols[0],
            art,
        );
    } else {
        frame.render_widget(
            Paragraph::new("♪\n(no cover)")
                .style(theme::dim())
                .alignment(Alignment::Center),
            cols[0],
        );
    }
}

// ── toast ───────────────────────────────────────────────────────────────

fn draw_toast(frame: &mut Frame, app: &App) {
    let Some(alpha) = app.toast_alpha() else {
        return;
    };
    let Some(msg) = app.toast.as_ref() else {
        return;
    };
    if app.popup.is_some() || app.screen != Screen::Browse {
        return;
    }
    let text = format::truncate(msg, 44);
    let w = (text.chars().count() as u16) + 4;
    let full = frame.area();
    if full.width < w + 2 || full.height < 5 {
        return;
    }
    // Bottom-right, just above the footer.
    let area = Rect::new(full.width - w - 1, full.height.saturating_sub(4), w, 3);
    let fg = theme::blend(theme::YELLOW, theme::SURFACE, 1.0 - alpha);
    let border = theme::blend(theme::ACCENT, theme::SURFACE, 1.0 - alpha);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme::SURFACE));
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(fg)))
            .block(block)
            .alignment(Alignment::Center),
        area,
    );
}

// ── login screen ─────────────────────────────────────────────────────────

fn draw_login(frame: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let mut brand = gradient_spans("≈ Tiders", theme::ACCENT2, theme::ACCENT);
    brand.push(Span::styled("  ·  Sign in to TIDAL", theme::dim()));
    frame.render_widget(Paragraph::new(Line::from(brand)), outer[0]);

    let box_area = centered_pct(70, 70, outer[1]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .title(Span::styled(" Device Login ", theme::accent()));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let mut lines: Vec<Line> = vec![Line::from("")];
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
                    "Press r to retry, or q to quit.",
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
        None => lines.push(Line::from(Span::styled("Preparing login…", theme::dim()))),
    }

    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        inner,
    );

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

// ── small helpers ─────────────────────────────────────────────────────────

fn spinner(tick: u64) -> String {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(tick as usize / 2) % FRAMES.len()].to_string()
}

/// An animated frequency-bar "spectrum" with a teal→green gradient.
fn spectrum_line(tick: u64, animate: bool, bars: usize) -> Line<'static> {
    const GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut spans = Vec::with_capacity(bars);
    for i in 0..bars {
        let h = if animate {
            let phase = (tick as usize + i * 2) % 14;
            if phase < 8 {
                phase
            } else {
                14 - phase
            }
        } else {
            0
        };
        let h = h.min(7);
        let color = theme::blend(theme::ACCENT, theme::GREEN, h as f32 / 7.0);
        spans.push(Span::styled(
            GLYPHS[h].to_string(),
            Style::default().fg(color),
        ));
    }
    Line::from(spans)
}

/// Horizontal marquee: scrolls `text` within `width` while playing.
fn marquee(text: &str, width: usize, tick: u64, animate: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    if width == 0 {
        return String::new();
    }
    if chars.len() <= width {
        return text.to_string();
    }
    if !animate {
        return format::truncate(text, width);
    }
    let sep = "   •   ";
    let loop_str: Vec<char> = text.chars().chain(sep.chars()).collect();
    let offset = (tick as usize / 3) % loop_str.len();
    let mut out = String::with_capacity(width);
    for k in 0..width {
        out.push(loop_str[(offset + k) % loop_str.len()]);
    }
    out
}

/// A left-to-right two-color gradient over the characters of `text`.
fn gradient_spans(text: &str, from: Color, to: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len().max(1);
    chars
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let t = i as f32 / (n - 1).max(1) as f32;
            Span::styled(
                c.to_string(),
                Style::default()
                    .fg(theme::blend(from, to, t))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// Shrink a rect by `(dx, dy)` on each side.
fn inset(area: Rect, dx: u16, dy: u16) -> Rect {
    Rect {
        x: area.x + dx,
        y: area.y + dy,
        width: area.width.saturating_sub(dx * 2),
        height: area.height.saturating_sub(dy * 2),
    }
}

/// Compute a centered rectangle sized `w`×`h` cells within `area`.
fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

/// Compute a centered rectangle sized `pct_w`% × `pct_h`% of `area`.
fn centered_pct(pct_w: u16, pct_h: u16, area: Rect) -> Rect {
    let w = area.width * pct_w.min(100) / 100;
    let h = area.height * pct_h.min(100) / 100;
    centered_rect(w, h, area)
}
