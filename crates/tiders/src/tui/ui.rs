//! Rendering for the Tiders TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap,
};
use ratatui::Frame;
use ratatui_image::{Resize, StatefulImage};

use tiders_core::format;
use tiders_core::lyrics;
use tiders_core::model::{HomeCardKind, TrackView};
use tiders_core::queue::{RepeatMode, ShuffleMode};
use tiders_core::spectrum::{self, EqTheme};
use tiders_core::PlayerStatus;

use super::app::{App, FavSection, LibSection, Popup, Screen, SearchScope, Tab, QUALITIES};
use super::theme;

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
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
    let area = inset(frame.area(), 2, 1);
    let np = super::anim::ease_out_cubic(app.np_progress.clamp(0.0, 1.0));
    // Interpolate the now-playing pane from a compact bar to a fullscreen stage.
    let compact = 8.0;
    let stage = (area.height as f32 * 0.72).max(18.0);
    let np_h = super::anim::lerp(compact, stage, np).round() as u16;
    let list_min = area.height.saturating_sub(np_h + 3).max(3);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(list_min),
            Constraint::Length(np_h),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);
    draw_tabs(frame, app, chunks[1]);
    if np > 0.85 {
        draw_now_playing_stage(frame, app, chunks[2].union(chunks[3]));
    } else {
        draw_list(frame, app, chunks[2]);
        draw_now_playing(frame, app, chunks[3]);
    }
    draw_footer(frame, app, chunks[4]);
}

fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
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

    let q = app.player.quality().clone();
    let qlabel = if q.sample_rate_hz.is_some() || q.codecs.is_some() {
        q.label()
    } else {
        app.settings.quality.short_label().to_string()
    };
    let right = format!(
        "{} · vol {}% · img:{}",
        qlabel,
        app.player.volume(),
        app.art.protocol_label()
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(right.chars().count() as u16 + 1),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(Line::from(left)), cols[0]);
    frame.render_widget(
        Paragraph::new(Span::styled(right, theme::dim())).alignment(Alignment::Right),
        cols[1],
    );
}

fn draw_tabs(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.input_mode {
        let label = if app.tab == Tab::Search && app.nav.is_empty() {
            "  Search  "
        } else {
            "  Filter  "
        };
        let hint = if app.tab == Tab::Search && app.nav.is_empty() {
            "  Enter catalog · ↑↓ move · Esc clear"
        } else {
            "  ↑↓ move · Enter open · Esc clear"
        };
        let line = Line::from(vec![
            Span::styled(label, Style::default().fg(theme::BG).bg(theme::ACCENT)),
            Span::raw(" "),
            Span::styled(&app.input, Style::default().fg(theme::FG)),
            Span::styled(
                if app.tick % 16 < 8 { "▏" } else { " " },
                Style::default().fg(theme::ACCENT),
            ),
            Span::styled(hint, theme::dim()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let mut spans = Vec::new();
    let mut x = area.x;
    for tab in [
        Tab::Search,
        Tab::Library,
        Tab::Mixes,
        Tab::Favorites,
        Tab::Queue,
    ] {
        let selected = app.tab == tab && app.nav.is_empty();
        let (fg, modifier) = if selected {
            (theme::ACCENT, Modifier::BOLD)
        } else {
            (theme::DIM, Modifier::empty())
        };
        let label = format!("  {}  ", tab.title());
        let w = label.chars().count() as u16;
        app.hits.tabs.push((Rect::new(x, area.y, w, 1), tab));
        spans.push(Span::styled(
            label,
            Style::default().fg(fg).add_modifier(modifier),
        ));
        spans.push(Span::styled("│", Style::default().fg(theme::BORDER)));
        x = x.saturating_add(w + 1);
    }
    if !app.nav.is_empty() {
        spans.push(Span::styled(
            format!("  {}  ", app.page_title()),
            Style::default()
                .fg(theme::ACCENT2)
                .add_modifier(Modifier::BOLD),
        ));
    } else if app.tab == Tab::Library {
        for sec in [LibSection::Playlists, LibSection::Mixes, LibSection::ForYou] {
            let on = app.lib_section == sec;
            let label = format!(" {} ", sec.title());
            let w = label.chars().count() as u16;
            app.hits.lib.push((Rect::new(x, area.y, w, 1), sec));
            spans.push(Span::styled(
                label,
                if on {
                    Style::default()
                        .fg(theme::BG)
                        .bg(theme::ACCENT2)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::ACCENT2)
                },
            ));
            x = x.saturating_add(w);
        }
    } else if app.tab == Tab::Favorites {
        for sec in [FavSection::Tracks, FavSection::Albums, FavSection::Artists] {
            let on = app.fav_section == sec;
            let label = format!(" {} ", sec.title());
            let w = label.chars().count() as u16;
            app.hits.fav.push((Rect::new(x, area.y, w, 1), sec));
            spans.push(Span::styled(
                label,
                if on {
                    Style::default()
                        .fg(theme::BG)
                        .bg(theme::ACCENT2)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::ACCENT2)
                },
            ));
            x = x.saturating_add(w);
        }
    } else if app.tab == Tab::Search {
        spans.push(Span::styled(
            format!(" [{}] ", app.search_scope.title()),
            Style::default().fg(theme::ACCENT2),
        ));
    }
    let _ = x;
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = if app.filter_active() {
        format!(
            " {} ({}/{}) ",
            app.page_title(),
            app.current_len(),
            app.unfiltered_len()
        )
    } else {
        format!(" {} ({}) ", app.page_title(), app.current_len())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .padding(Padding::horizontal(1))
        .title(Span::styled(title, theme::accent()));

    let inner = block.inner(area);
    app.hits.list = Some(inner);
    app.hits.list_offset = app.list_state.offset();

    if app.current_len() == 0 {
        let hint = empty_hint(app);
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

    if app.showing_tracks() || !app.nav.is_empty() {
        let tracks = app.current_tracks();
        let width = area.width.saturating_sub(4) as usize;
        let now_playing_id = app.player.now_playing().map(|t| t.id);
        let items: Vec<ListItem> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let hit = app.hit_at(i);
                track_row(i, t, width, now_playing_id == Some(t.id), hit)
            })
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(theme::selected())
            .highlight_symbol("▍ ");
        frame.render_stateful_widget(list, area, &mut app.list_state);
        return;
    }

    let items = collection_rows(app);
    let list = List::new(items)
        .block(block)
        .highlight_style(theme::selected())
        .highlight_symbol("▍ ");
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn empty_hint(app: &App) -> String {
    if app.filter_active() {
        return format!("No matches for “{}”.", app.input.trim());
    }
    if !app.nav.is_empty() {
        return "Nothing in this collection.".into();
    }
    match app.tab {
        Tab::Search => "No results yet — press / to filter, Enter to search the catalog.".into(),
        Tab::Library => {
            "Library is empty — playlists, mixes and For You load after sign-in.".into()
        }
        Tab::Mixes => "No mixes yet — they load after sign-in. Click Mixes or press 3.".into(),
        Tab::Favorites => "No favorites loaded — press f to reload.".into(),
        Tab::Queue => "The queue is empty — pick a track and press Enter to play.".into(),
    }
}

fn collection_rows(app: &App) -> Vec<ListItem<'static>> {
    match app.tab {
        Tab::Search => match app.search_scope {
            SearchScope::Albums => map_visible(&app.results.albums, app, |a, hit| {
                row(
                    &a.title,
                    &a.artist,
                    "album",
                    hit_title_indices(a.title.len(), hit),
                )
            }),
            SearchScope::Artists => map_visible(&app.results.artists, app, |a, hit| {
                row(&a.name, "artist", "", hit_indices(hit))
            }),
            SearchScope::Playlists => map_visible(&app.results.playlists, app, |p, hit| {
                row(
                    &p.title,
                    &format!("{} tracks", p.tracks),
                    "playlist",
                    hit_title_indices(p.title.len(), hit),
                )
            }),
            SearchScope::Tracks => Vec::new(),
        },
        Tab::Library => match app.lib_section {
            LibSection::Playlists => map_visible(&app.playlists, app, |p, hit| {
                row(
                    &p.title,
                    &format!("{} tracks", p.tracks),
                    "playlist",
                    hit_title_indices(p.title.len(), hit),
                )
            }),
            LibSection::Mixes => map_visible(&app.mixes, app, |m, hit| {
                row(
                    &m.title,
                    &m.subtitle,
                    "mix",
                    hit_title_indices(m.title.len(), hit),
                )
            }),
            LibSection::ForYou => map_visible(&app.for_you, app, |c, hit| {
                let kind = match c.kind {
                    HomeCardKind::Mix { .. } => "mix",
                    HomeCardKind::Playlist { .. } => "playlist",
                    HomeCardKind::Album { .. } => "album",
                    HomeCardKind::Artist { .. } => "artist",
                };
                row(
                    &c.title,
                    &c.subtitle,
                    kind,
                    hit_title_indices(c.title.len(), hit),
                )
            }),
        },
        Tab::Mixes => map_visible(&app.mixes, app, |m, hit| {
            row(
                &m.title,
                &m.subtitle,
                "mix",
                hit_title_indices(m.title.len(), hit),
            )
        }),
        Tab::Favorites => match app.fav_section {
            FavSection::Albums => map_visible(&app.fav_albums, app, |a, hit| {
                row(
                    &a.title,
                    a.artist.as_str(),
                    "album",
                    hit_title_indices(a.title.len(), hit),
                )
            }),
            FavSection::Artists => map_visible(&app.fav_artists, app, |a, hit| {
                row(&a.name, "artist", "", hit_indices(hit))
            }),
            FavSection::Tracks => Vec::new(),
        },
        Tab::Queue => Vec::new(),
    }
}

fn map_visible<'a, T>(
    items: &'a [T],
    app: &'a App,
    mut f: impl FnMut(&'a T, Option<&'a super::filter::Hit>) -> ListItem<'static>,
) -> Vec<ListItem<'static>> {
    (0..app.current_len())
        .filter_map(|i| items.get(app.orig_at(i)).map(|item| f(item, app.hit_at(i))))
        .collect()
}

fn hit_indices(hit: Option<&super::filter::Hit>) -> Vec<usize> {
    hit.map(|h| h.indices.clone()).unwrap_or_default()
}

fn hit_title_indices(title_len: usize, hit: Option<&super::filter::Hit>) -> Vec<usize> {
    let Some(hit) = hit else {
        return Vec::new();
    };
    super::filter::split_highlights(title_len, 1, &hit.indices).0
}

fn row(title: &str, subtitle: &str, kind: &str, title_hits: Vec<usize>) -> ListItem<'static> {
    let hit_style = Style::default()
        .fg(theme::ACCENT)
        .add_modifier(Modifier::BOLD);
    let base = Style::default().fg(theme::FG);
    let mut spans = vec![Span::styled(
        format!("{kind:<8} "),
        Style::default().fg(theme::ACCENT2),
    )];
    spans.extend(super::filter::highlight(
        title,
        &title_hits,
        base,
        hit_style,
    ));
    if !subtitle.is_empty() {
        spans.push(Span::styled(format!("  {subtitle}"), theme::dim()));
    }
    ListItem::new(Line::from(spans))
}

fn track_row<'a>(
    index: usize,
    track: &'a TrackView,
    width: usize,
    is_current: bool,
    hit: Option<&super::filter::Hit>,
) -> ListItem<'a> {
    let dur = track.duration();
    let text_budget = width.saturating_sub(4 + 7 + 8 + 2).max(8);
    let title_budget = (text_budget * 3) / 5;
    let artist_budget = text_budget.saturating_sub(title_budget);
    let marker = if is_current { "♪ " } else { "  " };
    let title = format::truncate(&track.title, title_budget);
    let artist = format::truncate(&track.artist, artist_budget.max(4));
    let (title_hits, artist_hits) = match hit {
        Some(h) => super::filter::split_highlights(track.title.len(), 1, &h.indices),
        None => (Vec::new(), Vec::new()),
    };
    let title_style = if is_current {
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::FG)
    };
    let hit_style = Style::default()
        .fg(theme::ACCENT)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::styled(marker, Style::default().fg(theme::GREEN)),
        Span::styled(format!("{:>2}. ", index + 1), theme::dim()),
    ];
    spans.extend(super::filter::highlight(
        &title,
        &title_hits,
        title_style,
        hit_style,
    ));
    spans.push(Span::styled("  ", Style::default()));
    spans.extend(super::filter::highlight(
        &artist,
        &artist_hits,
        theme::dim(),
        hit_style,
    ));
    if let Some(badge) = track.quality_badge() {
        spans.push(Span::styled(
            format!(" {badge}"),
            Style::default().fg(theme::ACCENT2),
        ));
    }
    if track.explicit {
        spans.push(Span::styled("  E", Style::default().fg(theme::MAGENTA)));
    }
    spans.push(Span::styled(format!("   {dur:>5}"), theme::dim()));
    ListItem::new(Line::from(spans))
}

fn draw_now_playing(frame: &mut Frame, app: &mut App, area: Rect) {
    let status = app.player.status();
    let np = app.player.now_playing().cloned();
    let volume = app.player.volume();
    let snap = app.player.snapshot();
    let elapsed = app.elapsed_secs();
    let progress = app.progress();
    let tick = app.tick;
    let has_art = app.now_art.is_some();
    let shuffle = app.player.shuffle();
    let repeat = app.player.repeat();
    let qlabel = app.player.quality().label();
    let show_spec = app.settings.show_spectrum;
    let theme_eq = app.settings.eq_theme;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Now Playing ", theme::dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let art_w = if has_art {
        inner.height.saturating_mul(2).min(inner.width / 3)
    } else {
        0
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_w), Constraint::Min(10)])
        .split(inner);
    let art_area = cols[0];
    let info_area = cols[1];

    let spec_h = if show_spec {
        (info_area.height / 2).clamp(1, 4)
    } else {
        0
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(spec_h),
            Constraint::Length(1),
            Constraint::Min(1),
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

            if spec_h > 0 {
                draw_spectrum(frame, app, rows[1], theme_eq);
            }

            let total = track.duration_secs as f64;
            draw_scrub(frame, app, rows[2], elapsed, total, progress);

            let pos = match snap.queue_position {
                Some(i) if snap.queue_len > 0 => format!("{}/{}", i + 1, snap.queue_len),
                _ => "0/0".into(),
            };
            let sh_style = if shuffle == ShuffleMode::Off {
                theme::dim()
            } else {
                Style::default().fg(theme::ACCENT)
            };
            let rp_style = if repeat == RepeatMode::Off {
                theme::dim()
            } else {
                Style::default().fg(theme::ACCENT)
            };
            let lyric = current_lyric(app, elapsed);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!(" {} {}  ", shuffle.icon(), shuffle.label()),
                        sh_style,
                    ),
                    Span::styled(format!("{} {}  ", repeat.icon(), repeat.label()), rp_style),
                    Span::styled(format!("vol {volume}%  "), theme::dim()),
                    Span::styled(format!("{pos}  "), Style::default().fg(theme::FG)),
                    Span::styled(qlabel, Style::default().fg(theme::ACCENT2)),
                    Span::styled(
                        lyric.map(|s| format!("  ·  {s}")).unwrap_or_default(),
                        theme::dim(),
                    ),
                ])),
                rows[3],
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

fn draw_now_playing_stage(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .title(Span::styled(" Now Playing ", theme::accent()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(38),
            Constraint::Percentage(37),
            Constraint::Percentage(25),
        ])
        .split(inner);

    // Cover + spectrum
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(6)])
        .split(cols[0]);
    if let Some(art) = app.now_art.as_mut() {
        frame.render_stateful_widget(
            StatefulImage::default().resize(Resize::Fit(None)),
            left[0],
            art,
        );
    } else {
        frame.render_widget(
            Paragraph::new("♪")
                .alignment(Alignment::Center)
                .style(theme::dim()),
            left[0],
        );
    }
    if app.settings.show_spectrum {
        draw_spectrum(frame, app, left[1], app.settings.eq_theme);
    }

    // Track info + lyrics
    let track = app.player.now_playing().cloned();
    let elapsed = app.elapsed_secs();
    let progress = app.progress();
    let status = app.player.status();
    let qlabel = app.player.quality().label();
    let mid = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .split(inset(cols[1], 1, 0));

    if let Some(track) = &track {
        let (icon, _) = match status {
            PlayerStatus::Playing => ("▶", theme::GREEN),
            PlayerStatus::Paused => ("⏸", theme::YELLOW),
            PlayerStatus::Stopped => ("⏹", theme::DIM),
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("{icon}  {}", track.title),
                    Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!(
                        "{}  ·  {}",
                        track.artist,
                        track.album.clone().unwrap_or_default()
                    ),
                    Style::default().fg(theme::ACCENT2),
                )),
                Line::from(Span::styled(qlabel, theme::dim())),
            ]),
            mid[0],
        );
        draw_scrub(
            frame,
            app,
            mid[1],
            elapsed,
            track.duration_secs as f64,
            progress,
        );
        draw_lyrics(frame, app, elapsed, mid[2]);
    } else {
        frame.render_widget(
            Paragraph::new("Nothing playing").style(theme::dim()),
            mid[0],
        );
    }

    // Mini queue
    let qblock = Block::default()
        .borders(Borders::LEFT)
        .border_style(theme::border())
        .title(Span::styled(" Queue ", theme::dim()));
    let qinner = qblock.inner(cols[2]);
    frame.render_widget(qblock, cols[2]);
    let items: Vec<ListItem> = app
        .player
        .queue()
        .items()
        .iter()
        .enumerate()
        .take(qinner.height as usize)
        .map(|(i, t)| {
            let cur = app.player.queue().cursor() == Some(i);
            ListItem::new(Span::styled(
                format::truncate(&format!("{}. {}", i + 1, t.title), qinner.width as usize),
                if cur {
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    theme::dim()
                },
            ))
        })
        .collect();
    frame.render_widget(List::new(items), qinner);
}

fn draw_lyrics(frame: &mut Frame, app: &App, elapsed: f64, area: Rect) {
    if app.lyrics.is_empty() {
        frame.render_widget(
            Paragraph::new("No synced lyrics for this track.")
                .style(theme::dim())
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let idx = lyrics::current_line_index(&app.lyrics, elapsed).unwrap_or(0);
    let start = idx.saturating_sub(area.height as usize / 2);
    let mut lines = Vec::new();
    for (i, line) in app.lyrics.iter().enumerate().skip(start) {
        if lines.len() >= area.height as usize {
            break;
        }
        let style = if i == idx {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else if i + 1 == idx || i == idx + 1 {
            Style::default().fg(theme::FG)
        } else {
            theme::dim()
        };
        lines.push(Line::from(Span::styled(line.text.clone(), style)));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_spectrum(frame: &mut Frame, app: &mut App, area: Rect, eq: EqTheme) {
    if area.height == 0 || area.width < 8 {
        return;
    }
    app.spectrum.set_bars(area.width as usize);
    let frame_spec = app.spectrum.frame();
    let rows = spectrum::render_rows(&frame_spec, area.height);
    let stops = eq.stops();
    let mut lines = Vec::new();
    for row in rows {
        let spans: Vec<Span> = row
            .into_iter()
            .map(|(ch, t)| {
                let color = gradient_stop(stops, t);
                Span::styled(ch.to_string(), Style::default().fg(color))
            })
            .collect();
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn gradient_stop(stops: [(u8, u8, u8); 4], t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let seg = t * (stops.len() - 1) as f32;
    let lo = (seg.floor() as usize).min(stops.len() - 2);
    let frac = seg - lo as f32;
    let (ar, ag, ab) = stops[lo];
    let (br, bg, bb) = stops[lo + 1];
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * frac).round() as u8;
    Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
}

fn current_lyric(app: &App, elapsed: f64) -> Option<String> {
    let idx = lyrics::current_line_index(&app.lyrics, elapsed)?;
    Some(app.lyrics.get(idx)?.text.clone())
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints: &[(&str, &str)] = if app.input_mode {
        if app.tab == Tab::Search && app.nav.is_empty() {
            &[
                ("Enter", "catalog"),
                ("↑↓", "move"),
                ("Esc", "clear"),
                ("C-u", "reset"),
            ]
        } else {
            &[
                ("Enter", "open"),
                ("↑↓", "move"),
                ("Esc", "clear"),
                ("C-u", "reset"),
            ]
        }
    } else if app.filter_active() {
        &[
            ("Esc", "clear filter"),
            ("/", "edit"),
            ("Enter", "open"),
            ("j/k", "move"),
        ]
    } else if app.now_playing_mode {
        &[
            ("Esc", "back"),
            ("Spc", "pause"),
            ("n/p", "next/prev"),
            ("s", "shuffle"),
            ("r", "repeat"),
            ("e", "eq"),
            ("q", "quit"),
        ]
    } else {
        &[
            ("/", "filter"),
            ("3", "mixes"),
            ("↵", "play"),
            ("Spc", "pause"),
            ("s", "shuffle"),
            ("r", "repeat"),
            ("m", "now playing"),
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

fn draw_popup(frame: &mut Frame, app: &mut App) {
    let factor = app.popup_factor().clamp(0.15, 1.0);
    let (base_w, base_h) = match app.popup {
        Some(Popup::Help) => (62u16, 82u16),
        Some(Popup::Detail(_)) => (70, 78),
        Some(Popup::Quality) => (52, 56),
        None => return,
    };
    let pct_w = ((base_w as f32) * factor) as u16;
    let pct_h = ((base_h as f32) * factor) as u16;
    let area = centered_pct(pct_w.max(18), pct_h.max(18), frame.area());
    frame.render_widget(Clear, area);

    enum Kind {
        Help,
        Quality,
        Detail(Box<TrackView>),
    }
    let kind = match &app.popup {
        Some(Popup::Help) => Kind::Help,
        Some(Popup::Quality) => Kind::Quality,
        Some(Popup::Detail(track)) => Kind::Detail(track.clone()),
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
    if factor < 0.88 {
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
        (
            "/",
            "filter the current list (FFF) — Enter searches the catalog",
        ),
        ("Tab / 1–5", "Search · Library · Mixes · Favorites · Queue"),
        ("t / S", "cycle search scope / library-favorites section"),
        ("Enter", "play or open the highlighted item"),
        ("a / A", "add track / add all to queue"),
        ("Space", "play / pause"),
        ("n / p", "next / previous"),
        ("← →", "seek ±10s"),
        ("s / r", "shuffle / repeat (off · all · one)"),
        ("R", "start radio from the focused track"),
        ("l", "love / unlove"),
        ("m", "now-playing mode (big cover, lyrics, mini queue)"),
        ("e / E", "toggle spectrum / cycle EQ theme"),
        ("d", "track details"),
        ("Q", "streaming quality"),
        ("x", "stop"),
        ("Esc", "back / close"),
        (
            "click",
            "tabs / sections / row (second click plays); drag to seek",
        ),
        ("q", "quit"),
    ];
    let mut lines = vec![Line::from("")];
    for (k, d) in rows {
        lines.push(Line::from(vec![key(k), desc(d)]));
    }
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
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(inset(area, 1, 1));
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
            Span::styled("Album     ", theme::dim()),
            Span::styled(album.clone(), theme::base()),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Length    ", theme::dim()),
        Span::styled(track.duration(), theme::base()),
    ]));
    if let Some(q) = track.audio_quality.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("Quality   ", theme::dim()),
            Span::styled(q.clone(), theme::base()),
        ]));
    }
    let live = app.player.quality().label();
    lines.push(Line::from(vec![
        Span::styled("Stream    ", theme::dim()),
        Span::styled(live, Style::default().fg(theme::ACCENT2)),
    ]));
    if let Some(bpm) = track.bpm {
        lines.push(Line::from(vec![
            Span::styled("BPM       ", theme::dim()),
            Span::styled(format!("{bpm:.0}"), theme::base()),
        ]));
    }
    if track.explicit {
        lines.push(Line::from(Span::styled(
            "Explicit",
            Style::default().fg(theme::MAGENTA),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), cols[1]);
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

fn draw_toast(frame: &mut Frame, app: &mut App) {
    let Some(alpha) = app.toast_alpha() else {
        return;
    };
    if app.popup.is_some() || app.screen != Screen::Browse {
        return;
    }
    let Some(toast) = app.toast.as_ref() else {
        return;
    };
    let title = format::truncate(&toast.title, 36);
    let subtitle = toast.subtitle.as_ref().map(|s| format::truncate(s, 36));
    let has_art = toast.art.is_some();
    let w = {
        let tw = title.chars().count();
        let sw = subtitle.as_ref().map(|s| s.chars().count()).unwrap_or(0);
        ((tw.max(sw) as u16) + 4 + if has_art { 8 } else { 0 }).clamp(18, 48)
    };
    let h = if has_art {
        5
    } else if subtitle.is_some() {
        4
    } else {
        3
    };
    let full = frame.area();
    if full.width < w + 4 || full.height < h + 3 {
        return;
    }
    let area = Rect::new(
        full.width.saturating_sub(w + 3),
        full.height.saturating_sub(h + 2),
        w,
        h,
    );
    let fg = theme::blend(theme::YELLOW, theme::SURFACE, 1.0 - alpha);
    let sub_fg = theme::blend(theme::ACCENT2, theme::SURFACE, 1.0 - alpha);
    let border = theme::blend(theme::ACCENT, theme::SURFACE, 1.0 - alpha);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme::SURFACE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut text_lines = vec![Line::from(Span::styled(
        title,
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    ))];
    if let Some(sub) = subtitle {
        text_lines.push(Line::from(Span::styled(sub, Style::default().fg(sub_fg))));
    }

    if has_art {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(6), Constraint::Min(6)])
            .split(inner);
        if let Some(art) = app.toast.as_mut().and_then(|t| t.art.as_mut()) {
            frame.render_stateful_widget(
                StatefulImage::default().resize(Resize::Fit(None)),
                cols[0],
                art,
            );
        }
        frame.render_widget(
            Paragraph::new(text_lines).alignment(Alignment::Left),
            cols[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(text_lines).alignment(Alignment::Center),
            inner,
        );
    }
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

fn spinner(tick: u64) -> String {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(tick as usize / 4) % FRAMES.len()].to_string()
}

/// Compact scrubber: `0:12 ──●──────── 3:45` capped at 28 bar cells so it
/// doesn't span the whole pane and jump a cell every second.
fn draw_scrub(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    elapsed: f64,
    total: f64,
    progress: f64,
) {
    if area.width < 12 {
        return;
    }
    let elapsed_s = format::duration(elapsed.max(0.0) as u64);
    let total_s = format::duration(total.max(0.0) as u64);
    let time_w = elapsed_s.len().max(total_s.len()).max(4) as u16;
    let bar_budget = area.width.saturating_sub(time_w * 2 + 2);
    let bar_w = bar_budget.clamp(8, 28);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(time_w),
            Constraint::Length(bar_w),
            Constraint::Length(time_w + 1),
        ])
        .split(area);
    app.hits.progress = Some(cols[1]);
    frame.render_widget(
        Paragraph::new(Span::styled(elapsed_s, theme::dim())),
        cols[0],
    );
    frame.render_widget(Paragraph::new(scrub_bar(progress, bar_w as usize)), cols[1]);
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {total_s}"), theme::dim()))
            .alignment(Alignment::Right),
        cols[2],
    );
}

fn scrub_bar(progress: f64, width: usize) -> Line<'static> {
    let progress = progress.clamp(0.0, 1.0);
    if width == 0 {
        return Line::from("");
    }
    let pos = ((progress * (width.saturating_sub(1)) as f64).round() as usize).min(width - 1);
    let mut spans = Vec::with_capacity(width);
    for i in 0..width {
        if i == pos {
            spans.push(Span::styled(
                "●",
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if i < pos {
            spans.push(Span::styled("─", Style::default().fg(theme::ACCENT)));
        } else {
            spans.push(Span::styled("─", theme::dim()));
        }
    }
    Line::from(spans)
}

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
    let offset = ((tick as usize) / 6) % loop_str.len();
    let mut out = String::with_capacity(width);
    for k in 0..width {
        out.push(loop_str[(offset + k) % loop_str.len()]);
    }
    out
}

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

fn inset(area: Rect, dx: u16, dy: u16) -> Rect {
    Rect {
        x: area.x + dx,
        y: area.y + dy,
        width: area.width.saturating_sub(dx * 2),
        height: area.height.saturating_sub(dy * 2),
    }
}

fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn centered_pct(pct_w: u16, pct_h: u16, area: Rect) -> Rect {
    let w = area.width * pct_w.min(100) / 100;
    let h = area.height * pct_h.min(100) / 100;
    centered_rect(w, h, area)
}
