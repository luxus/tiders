//! Rendering for the Tiders TUI.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, Padding, Paragraph, Row, Table,
    TableState, Wrap,
};
use ratatui::Frame;
use ratatui_image::{Resize, StatefulImage};

use tiders_core::format;
use tiders_core::lyrics;
use tiders_core::model::{HomeCardKind, TrackView};
use tiders_core::queue::{RepeatMode, ShuffleMode};
use tiders_core::spectrum::{self, EqTheme};
use tiders_core::PlayerStatus;

use super::app::{
    App, ContextAction, FavSection, Focus, Popup, Screen, SearchScope, SortKey, Tab, QUALITIES,
};
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
    let area = inset(frame.area(), 1, 0);
    let np = super::anim::ease_out_cubic(app.np_progress.clamp(0.0, 1.0));
    let compact = 10.0;
    let stage = (area.height as f32 * 0.78).max(18.0);
    let np_h = super::anim::lerp(compact, stage, np).round() as u16;
    let list_min = area.height.saturating_sub(np_h + 2).max(3);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(list_min),
            Constraint::Length(np_h),
            Constraint::Length(1),
        ])
        .split(area);

    let sidebar_w = if app.settings.sidebar_visible {
        22.min(rows[0].width / 3).max(16)
    } else {
        3
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_w), Constraint::Min(20)])
        .split(rows[0]);

    draw_sidebar(frame, app, cols[0]);
    if np > 0.85 {
        draw_now_playing_stage(frame, app, cols[1].union(rows[1]));
    } else {
        draw_main(frame, app, cols[1]);
        draw_now_playing(frame, app, rows[1]);
    }
    draw_footer(frame, app, rows[2]);
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Sidebar;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            theme::border_focused()
        } else {
            theme::border()
        })
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.settings.sidebar_visible {
        app.hits.sidebar_toggle = Some(inner);
        frame.render_widget(
            Paragraph::new("»")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme::ACCENT)),
            inner,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(Tab::all().len() as u16),
            Constraint::Min(4),
        ])
        .split(inner);

    let toggle = Rect::new(chunks[0].x, chunks[0].y, chunks[0].width.min(8), 1);
    app.hits.sidebar_toggle = Some(toggle);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("« ", Style::default().fg(theme::ACCENT)),
            Span::styled("hide", theme::dim()),
        ])),
        chunks[0],
    );

    let mut nav_lines = Vec::new();
    for (i, tab) in Tab::all().iter().enumerate() {
        let y = chunks[1].y + i as u16;
        if y >= chunks[1].y + chunks[1].height {
            break;
        }
        let rect = Rect::new(chunks[1].x, y, chunks[1].width, 1);
        app.hits.tabs.push((rect, *tab));
        let on = app.tab == *tab && app.nav.is_empty();
        let marker = if on { "● " } else { "  " };
        nav_lines.push(Line::from(vec![
            Span::styled(
                marker,
                Style::default().fg(if on { theme::ACCENT } else { theme::DIM }),
            ),
            Span::styled(
                tab.title(),
                if on {
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    theme::base()
                },
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(nav_lines), chunks[1]);

    let mut pl_lines = vec![Line::from(Span::styled("Playlists", theme::dim()))];
    let start_y = chunks[2].y + 1;
    for (i, p) in app.playlists.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= chunks[2].y + chunks[2].height {
            break;
        }
        pl_lines.push(Line::from(Span::styled(
            format::truncate(&p.title, chunks[2].width.saturating_sub(1) as usize),
            if app.tab == Tab::Playlists
                && app.nav.is_empty()
                && app.list_state.selected() == Some(i)
            {
                Style::default().fg(theme::ACCENT)
            } else {
                theme::base()
            },
        )));
        app.hits
            .playlists
            .push((Rect::new(chunks[2].x, y, chunks[2].width, 1), i));
    }
    if app.playlists.is_empty() {
        pl_lines.push(Line::from(Span::styled("  (none yet)", theme::dim())));
    }
    frame.render_widget(Paragraph::new(pl_lines), chunks[2]);
}

fn draw_main(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut constraints = Vec::new();
    if app.input_mode || app.filter_active() {
        constraints.push(Constraint::Length(1));
    }
    if app.tab == Tab::Favorites && app.nav.is_empty() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(3));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut i = 0;
    if app.input_mode || app.filter_active() {
        draw_filter_bar(frame, app, chunks[i]);
        i += 1;
    }
    if app.tab == Tab::Favorites && app.nav.is_empty() {
        draw_fav_sections(frame, app, chunks[i]);
        i += 1;
    }
    if app.artist_page() {
        draw_artist_page(frame, app, chunks[i]);
    } else {
        draw_list(frame, app, chunks[i]);
    }
}

fn draw_filter_bar(frame: &mut Frame, app: &App, area: Rect) {
    let label = if app.tab == Tab::Search && app.nav.is_empty() && app.input_mode {
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
            if app.input_mode && app.tick % 16 < 8 {
                "▏"
            } else {
                " "
            },
            Style::default().fg(theme::ACCENT),
        ),
        Span::styled(hint, theme::dim()),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_fav_sections(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut spans = Vec::new();
    let mut x = area.x;
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
    if app.tab == Tab::Search {
        spans.push(Span::styled(
            format!(" [{}] ", app.search_scope.title()),
            Style::default().fg(theme::ACCENT2),
        ));
    }
    let _ = x;
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_artist_page(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(6)])
        .split(area);

    let header = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .padding(Padding::new(1, 1, 0, 0))
        .title(Span::styled(
            format!(" {} ", app.page_title()),
            theme::accent(),
        ));
    let inner = header.inner(chunks[0]);
    frame.render_widget(header, chunks[0]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(18),
            Constraint::Length(2),
            Constraint::Min(20),
        ])
        .split(inner);
    if let Some(art) = app.page_art.as_mut() {
        frame.render_stateful_widget(
            StatefulImage::default().resize(Resize::Fit(None)),
            cols[0],
            art,
        );
    } else {
        frame.render_widget(
            Paragraph::new("♪")
                .alignment(Alignment::Center)
                .style(theme::dim()),
            cols[0],
        );
    }
    let name = app
        .page_artist
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| app.page_title());
    let mut lines = vec![
        Line::from(Span::styled(
            name,
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "{} top songs · {} albums",
                app.page_tracks.len(),
                app.page_albums.len()
            ),
            theme::dim(),
        )),
        Line::from(""),
    ];
    if !app.page_bio.is_empty() {
        lines.push(Line::from(Span::styled(
            format::truncate(&app.page_bio.replace('\n', " "), 400),
            theme::base(),
        )));
    }
    let info_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(cols[2]);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        info_rows[0],
    );
    let mut album_spans = vec![Span::styled("Albums  ", theme::dim())];
    let mut x = info_rows[1].x + 8;
    for (i, album) in app.page_albums.iter().take(6).enumerate() {
        let label = format!(" {} ", format::truncate(&album.title, 18));
        let w = label.chars().count() as u16;
        app.hits
            .albums
            .push((Rect::new(x, info_rows[1].y, w, 1), i));
        album_spans.push(Span::styled(
            label,
            Style::default().fg(theme::ACCENT2).bg(theme::SURFACE),
        ));
        x = x.saturating_add(w + 1);
    }
    if app.page_albums.is_empty() {
        album_spans.push(Span::styled("none yet", theme::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(album_spans)), info_rows[1]);

    draw_track_table(frame, app, chunks[1]);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.showing_tracks() {
        draw_track_table(frame, app, area);
        return;
    }

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
        frame.render_widget(
            Paragraph::new(empty_hint(app))
                .style(theme::dim())
                .alignment(Alignment::Center)
                .block(block)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let items = collection_rows(app);
    let list = List::new(items)
        .block(block)
        .highlight_style(theme::selected())
        .highlight_symbol("▍ ");
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_track_table(frame: &mut Frame, app: &mut App, area: Rect) {
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
    let sort_mark = |key: SortKey| {
        if app.sort_key == key {
            if app.sort_asc {
                "↑"
            } else {
                "↓"
            }
        } else {
            " "
        }
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .padding(Padding::horizontal(1))
        .title(Span::styled(title, theme::accent()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.current_len() == 0 {
        frame.render_widget(
            Paragraph::new(empty_hint(app))
                .style(theme::dim())
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let header_h = 1u16;
    let body = Rect {
        x: inner.x,
        y: inner.y.saturating_add(header_h),
        width: inner.width,
        height: inner.height.saturating_sub(header_h),
    };
    app.hits.list = Some(body);
    app.hits.list_offset = app.list_state.offset();

    let col_w = [
        inner.width.saturating_mul(34) / 100,
        inner.width.saturating_mul(24) / 100,
        inner.width.saturating_mul(28) / 100,
        inner
            .width
            .saturating_sub(inner.width.saturating_mul(86) / 100),
    ];
    let mut hx = inner.x;
    for (key, w) in [
        (SortKey::Title, col_w[0]),
        (SortKey::Artist, col_w[1]),
        (SortKey::Album, col_w[2]),
        (SortKey::Time, col_w[3]),
    ] {
        app.hits
            .sorts
            .push((Rect::new(hx, inner.y, w.max(1), 1), key));
        hx = hx.saturating_add(w);
    }

    let now_playing_id = app.player.now_playing().map(|t| t.id);
    let tracks = app.current_tracks();
    let header = Row::new([
        Cell::from(format!("Title {}", sort_mark(SortKey::Title))),
        Cell::from(format!("Artist {}", sort_mark(SortKey::Artist))),
        Cell::from(format!("Album {}", sort_mark(SortKey::Album))),
        Cell::from(format!("Time {}", sort_mark(SortKey::Time))),
    ])
    .style(
        Style::default()
            .fg(theme::ACCENT2)
            .add_modifier(Modifier::BOLD),
    );

    let rows = tracks.iter().map(|t| {
        let current = now_playing_id == Some(t.id);
        let style = if current {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::base()
        };
        let marker = if current { "♪ " } else { "  " };
        Row::new([
            Cell::from(format!("{marker}{}", t.title)).style(style),
            Cell::from(t.artist.clone()),
            Cell::from(t.album.clone().unwrap_or_default()),
            Cell::from(t.duration()),
        ])
        .style(if current {
            Style::default().fg(theme::ACCENT)
        } else {
            Style::default()
        })
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(34),
            Constraint::Percentage(24),
            Constraint::Percentage(28),
            Constraint::Percentage(14),
        ],
    )
    .header(header)
    .row_highlight_style(theme::selected())
    .highlight_symbol("▍ ");

    let mut state = TableState::default()
        .with_selected(app.list_state.selected())
        .with_offset(app.list_state.offset());
    frame.render_stateful_widget(table, inner, &mut state);
    *app.list_state.offset_mut() = state.offset();
    app.list_state.select(state.selected());
}

fn empty_hint(app: &App) -> String {
    if app.filter_active() {
        return format!("No matches for “{}”.", app.input.trim());
    }
    if !app.nav.is_empty() {
        return "Nothing in this collection.".into();
    }
    if !app.signed_in() {
        return "Sign in to load this section.".into();
    }
    if app.loading {
        return "Loading…".into();
    }
    match app.tab {
        Tab::Search => "No results yet — press / to filter, Enter to search the catalog.".into(),
        Tab::Home => app
            .home_error
            .clone()
            .unwrap_or_else(|| "For You is empty. Press f to reload.".into()),
        Tab::Mixes => app
            .home_error
            .clone()
            .map(|e| format!("No mixes yet ({e}). Press f to reload."))
            .unwrap_or_else(|| "No mixes yet. Press f to reload.".into()),
        Tab::Library => "No saved songs yet. Heart a track with l (or the context menu).".into(),
        Tab::Playlists => "No playlists yet. Press f to reload.".into(),
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
        Tab::Home => map_visible(&app.for_you, app, |c, hit| {
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
        Tab::Mixes => map_visible(&app.mixes, app, |m, hit| {
            row(
                &m.title,
                &m.subtitle,
                "mix",
                hit_title_indices(m.title.len(), hit),
            )
        }),
        Tab::Playlists => map_visible(&app.playlists, app, |p, hit| {
            row(
                &p.title,
                &format!("{} tracks", p.tracks),
                "playlist",
                hit_title_indices(p.title.len(), hit),
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
        Tab::Library | Tab::Queue => Vec::new(),
    }
}

fn map_visible<'a, T>(
    items: &'a [T],
    app: &'a App,
    mut f: impl FnMut(&'a T, Option<&'a super::filter::Hit>) -> ListItem<'static>,
) -> Vec<ListItem<'static>> {
    let order = app.view_indices();
    order
        .into_iter()
        .filter_map(|src| items.get(src).map(|item| f(item, app.hit_for_source(src))))
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
    let loved = np.as_ref().is_some_and(|t| app.loved.contains(&t.id));
    let up_next = app.player.queue().peek_next().cloned();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .padding(Padding::new(2, 2, 1, 1))
        .title(Span::styled(" Now Playing ", theme::dim()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let art_side = if has_art {
        inner.height.saturating_mul(2).min(inner.width / 4).max(8)
    } else {
        0
    };
    let up_w = if inner.width > 56 { 28 } else { 0 };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(art_side),
            Constraint::Length(if art_side > 0 { 6 } else { 0 }),
            Constraint::Min(16),
            Constraint::Length(if up_w > 0 { 2 } else { 0 }),
            Constraint::Length(up_w),
        ])
        .split(inner);
    let art_area = cols[0];
    let info_area = cols[2];
    let up_area = cols[4];

    let spec_h = if show_spec {
        (info_area.height / 2).clamp(1, 3)
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
            let user = app
                .service
                .as_ref()
                .and_then(|s| s.username())
                .unwrap_or_default();
            let loading = if app.loading {
                format!("  {} ", spinner(app.tick))
            } else {
                String::new()
            };
            let lyric = current_lyric(app, elapsed);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        if loved { " ★ fav  " } else { " ☆ fav  " },
                        if loved {
                            Style::default().fg(theme::ACCENT)
                        } else {
                            theme::dim()
                        },
                    ),
                    Span::styled(
                        format!("{} {}  ", shuffle.icon(), shuffle.label()),
                        sh_style,
                    ),
                    Span::styled(format!("{} {}  ", repeat.icon(), repeat.label()), rp_style),
                    Span::styled(format!("vol {volume}%  "), theme::dim()),
                    Span::styled(format!("{pos}  "), Style::default().fg(theme::FG)),
                    Span::styled(qlabel, Style::default().fg(theme::ACCENT2)),
                    Span::styled(loading, Style::default().fg(theme::ACCENT2)),
                    Span::styled(
                        if user.is_empty() {
                            String::new()
                        } else {
                            format!("  {user}")
                        },
                        theme::dim(),
                    ),
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

    if art_area.width > 4 && art_area.height > 1 {
        // Keep the kitty/sixel bitmap inside this rect so it cannot crowd
        // the title. Extra right inset is the gap the image protocol needs.
        let padded = Rect {
            x: art_area.x.saturating_add(1),
            y: art_area.y,
            width: art_area.width.saturating_sub(3),
            height: art_area.height,
        };
        if let Some(art) = app.now_art.as_mut() {
            frame.render_stateful_widget(
                StatefulImage::default().resize(Resize::Fit(None)),
                padded,
                art,
            );
        }
    }

    if up_w > 0 {
        app.hits.up_next = Some(up_area);
        let next_title = up_next
            .as_ref()
            .map(|t| format::truncate(&t.title, up_area.width.saturating_sub(2) as usize))
            .unwrap_or_else(|| "—".into());
        let next_artist = up_next
            .as_ref()
            .map(|t| format::truncate(&t.artist, up_area.width.saturating_sub(2) as usize))
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("Up Next", theme::dim())),
                Line::from(Span::styled(
                    next_title,
                    Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    next_artist,
                    Style::default().fg(theme::ACCENT2),
                )),
            ]),
            up_area,
        );
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
            Constraint::Percentage(34),
            Constraint::Length(4),
            Constraint::Percentage(36),
            Constraint::Percentage(26),
        ])
        .split(inner);

    // Cover + spectrum — extra inset so the bitmap cannot crowd the title.
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(6)])
        .split(inset(cols[0], 2, 1));
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
        .split(inset(cols[2], 1, 0));

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

    // Mini queue — clickable, scrollable, current track selected
    let qblock = Block::default()
        .borders(Borders::LEFT)
        .border_style(if app.focus == Focus::NpQueue {
            theme::border_focused()
        } else {
            theme::border()
        })
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Queue ", theme::dim()));
    let qinner = qblock.inner(cols[3]);
    frame.render_widget(qblock, cols[3]);
    app.hits.queue = Some(qinner);
    if app.np_queue_state.selected().is_none() {
        app.sync_np_queue_selection();
    }
    app.hits.queue_offset = app.np_queue_state.offset();
    let cursor = app.player.queue().cursor();
    let items: Vec<ListItem> = app
        .player
        .queue()
        .items()
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let cur = cursor == Some(i);
            ListItem::new(Span::styled(
                format::truncate(
                    &format!("{}{}. {}", if cur { "▶ " } else { "  " }, i + 1, t.title),
                    qinner.width as usize,
                ),
                if cur {
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    theme::base()
                },
            ))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(theme::selected())
        .highlight_symbol("▍ ");
    frame.render_stateful_widget(list, qinner, &mut app.np_queue_state);
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
            ("j/k", "queue"),
            ("Enter", "play"),
            ("Spc", "pause"),
            ("n/p", "next/prev"),
            ("q", "quit"),
        ]
    } else {
        &[
            ("/", "filter"),
            ("b", "sidebar"),
            ("o", "sort"),
            ("↵", "play"),
            ("Spc", "pause"),
            ("m", "now playing"),
            ("f", "reload"),
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
    if matches!(app.popup, Some(Popup::Context(_))) {
        draw_context_menu(frame, app);
        return;
    }
    let factor = app.popup_factor().clamp(0.15, 1.0);
    let (base_w, base_h) = match app.popup {
        Some(Popup::Help) => (62u16, 82u16),
        Some(Popup::Detail(_)) => (70, 78),
        Some(Popup::Quality) => (52, 56),
        Some(Popup::Context(_)) | None => return,
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
        Some(Popup::Context(_)) | None => return,
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

fn draw_context_menu(frame: &mut Frame, app: &mut App) {
    let (_loved, cursor, mx, my, labels) = {
        let Some(Popup::Context(menu)) = &app.popup else {
            return;
        };
        let loved = app.loved.contains(&menu.track.id);
        let labels: Vec<String> = ContextAction::all()
            .iter()
            .map(|a| format!(" {}", a.label(loved)))
            .collect();
        (loved, menu.cursor, menu.x, menu.y, labels)
    };
    let width = labels
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(18)
        .max(18) as u16
        + 2;
    let height = labels.len() as u16 + 2;
    let full = frame.area();
    let x = mx.min(full.width.saturating_sub(width));
    let y = my.min(full.height.saturating_sub(height));
    let area = Rect::new(x, y, width, height);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border_focused())
        .style(Style::default().bg(theme::SURFACE))
        .title(Span::styled(" Track ", theme::accent()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.hits.context.clear();
    app.hits.context_area = Some(area);
    let mut lines = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        // Full menu width (including side borders) so a click on the edge still
        // hits the row instead of falling through and closing the menu.
        let row_y = inner.y + i as u16;
        let rect = Rect::new(area.x, row_y, area.width, 1);
        app.hits.context.push((rect, i));
        let style = if i == cursor {
            theme::selected()
        } else {
            theme::base()
        };
        lines.push(Line::from(Span::styled(label.clone(), style)));
    }
    frame.render_widget(Paragraph::new(lines), inner);
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
        (
            "Tab / 1–7",
            "Search · For You · Mixes · Library · Playlists · Favorites · Queue",
        ),
        ("b", "show / hide the left sidebar"),
        ("o", "cycle table sort (title · artist · album · time)"),
        ("t / S", "cycle search scope / favorites section"),
        ("Enter", "play or open the highlighted item"),
        (
            "c / right-click",
            "track menu: artist · album · favorite · like · don't like",
        ),
        ("a / A", "add track / add all to queue"),
        ("Space", "play / pause"),
        ("n / p", "next / previous"),
        ("← →", "seek ±10s"),
        ("s / r", "shuffle / repeat (off · all · one)"),
        ("R", "start radio from the focused track"),
        ("l", "love / unlove"),
        (
            "m",
            "now-playing mode (big cover, lyrics, scrollable queue)",
        ),
        ("e / E", "toggle spectrum / cycle EQ theme"),
        ("d", "track details"),
        ("Q", "streaming quality"),
        ("f", "reload library, mixes, and For You"),
        ("x", "stop"),
        ("Esc", "back / close"),
        (
            "click",
            "sidebar / row / queue / sort headers; wheel scrolls; drag to seek",
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
    let Some(area) = toast_rect(full, w, h) else {
        return;
    };
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

/// Top-right toast slot, with a one-cell margin from the edges of `full`.
fn toast_rect(full: Rect, w: u16, h: u16) -> Option<Rect> {
    if full.width < w + 4 || full.height < h + 3 {
        return None;
    }
    Some(Rect::new(
        full.x + full.width.saturating_sub(w + 3),
        full.y.saturating_add(1),
        w,
        h,
    ))
}

#[cfg(test)]
mod tests {
    use super::toast_rect;
    use ratatui::layout::Rect;

    #[test]
    fn toast_sits_top_right() {
        let area = toast_rect(Rect::new(0, 0, 80, 24), 20, 4).unwrap();
        assert_eq!(area.x, 57);
        assert_eq!(area.y, 1);
        assert_eq!(area.width, 20);
        assert_eq!(area.height, 4);
    }

    #[test]
    fn toast_honours_frame_origin() {
        let area = toast_rect(Rect::new(10, 5, 80, 24), 20, 4).unwrap();
        assert_eq!(area.x, 67);
        assert_eq!(area.y, 6);
    }
}
