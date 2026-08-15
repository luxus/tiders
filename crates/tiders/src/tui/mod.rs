//! The interactive terminal UI.

mod anim;
mod app;
mod art;
mod filter;
mod hits;
mod theme;
mod ui;

pub use app::App;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use tiders_core::config::Config;

use app::{Focus, Popup, Screen, Tab, FRAME};
use art::ArtManager;
use hits::Hit;

/// Launch the TUI against the given config, restoring the terminal on exit.
pub async fn run(config: Config) -> Result<()> {
    let art = ArtManager::new();
    let mut app = App::bootstrap(config, art).await?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

/// Dedicated OS thread for input — grok-build's workaround for crossterm
/// stranding `EventStream` wakers when the future is dropped inside `select!`.
fn spawn_input_thread(tx: mpsc::UnboundedSender<Event>, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("tiders-input".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match crossterm::event::poll(Duration::from_millis(16)) {
                    Ok(true) => match crossterm::event::read() {
                        Ok(Event::Key(key)) if key.kind != KeyEventKind::Press => {}
                        Ok(ev) => {
                            if tx.send(ev).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    },
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        })
        .ok();
}

async fn event_loop<B: ratatui::backend::Backend + Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let (key_tx, mut key_rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    spawn_input_thread(key_tx, Arc::clone(&stop));

    let mut last = Instant::now();
    let mut last_idle_draw = Instant::now();
    let mut dirty = true;

    let result = loop {
        if dirty {
            let _ = execute!(terminal.backend_mut(), BeginSynchronizedUpdate);
            terminal.draw(|frame| ui::draw(frame, app))?;
            let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
            dirty = false;
        }

        // 120 Hz only while something is animating. Playback used to pin the
        // loop there (~30% CPU in the terminal) even with nothing moving.
        let wait = if app.needs_frames() {
            FRAME
        } else {
            app.playback_redraw_every()
        };

        tokio::select! {
            biased;
            maybe = key_rx.recv() => {
                match maybe {
                    Some(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        handle_key(app, key).await;
                        dirty = true;
                    }
                    Some(Event::Mouse(mouse)) => {
                        handle_mouse(app, mouse).await;
                        dirty = true;
                    }
                    Some(Event::Resize(_, _)) => {
                        dirty = true;
                    }
                    Some(_) => {}
                    None => break Ok(()),
                }
            }
            _ = tokio::time::sleep(wait) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last).as_secs_f32();
                last = now;
                app.on_frame(dt).await;
                if app.needs_frames() || last_idle_draw.elapsed() >= app.playback_redraw_every() {
                    dirty = true;
                    last_idle_draw = now;
                }
            }
        }

        if app.should_quit {
            break Ok(());
        }
    };

    stop.store(true, Ordering::Relaxed);
    result
}

async fn handle_key(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit();
        return;
    }

    match app.screen {
        Screen::Login => handle_login_key(app, key),
        Screen::Browse => {
            if app.popup_open() {
                handle_popup_key(app, key).await;
            } else {
                handle_browse_key(app, key).await;
            }
        }
    }
}

fn handle_login_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        KeyCode::Char('o') => app.open_login_url(),
        KeyCode::Char('r') => app.start_login(),
        _ => {}
    }
}

async fn handle_popup_key(app: &mut App, key: KeyEvent) {
    match &app.popup {
        Some(Popup::Quality) => match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.quality_move(-1),
            KeyCode::Down | KeyCode::Char('j') => app.quality_move(1),
            KeyCode::Enter => app.apply_quality(),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => app.close_popup(),
            _ => {}
        },
        Some(Popup::Context(_)) => match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.context_move(-1),
            KeyCode::Down | KeyCode::Char('j') => app.context_move(1),
            KeyCode::Enter => app.context_activate().await,
            KeyCode::Esc | KeyCode::Char('q') => app.close_popup(),
            _ => {}
        },
        _ => {
            if matches!(
                key.code,
                KeyCode::Esc
                    | KeyCode::Enter
                    | KeyCode::Char('q')
                    | KeyCode::Char('?')
                    | KeyCode::Char('d')
            ) {
                app.close_popup();
            }
        }
    }
}

async fn handle_browse_key(app: &mut App, key: KeyEvent) {
    if app.input_mode {
        match key.code {
            KeyCode::Enter => {
                if app.tab == Tab::Search && app.nav.is_empty() && !app.input.trim().is_empty() {
                    app.submit_search().await;
                } else {
                    app.input_mode = false;
                    app.activate().await;
                }
            }
            KeyCode::Esc => {
                if !app.input.is_empty() {
                    app.clear_filter();
                    app.select_first();
                } else {
                    app.input_mode = false;
                }
            }
            KeyCode::Backspace => {
                app.input.pop();
                app.recompute_filter();
                app.select_first();
            }
            KeyCode::Up => app.move_selection(-1),
            KeyCode::Down => app.move_selection(1),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.clear_filter();
                app.select_first();
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                app.input.push(c);
                app.recompute_filter();
                app.select_first();
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            if app.filter_active() {
                app.clear_filter();
                app.select_first();
            } else if !app.go_back() {
                app.quit();
            }
        }
        KeyCode::Char('q') => app.quit(),
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('d') => app.open_detail().await,
        KeyCode::Char('Q') => app.open_quality(),
        KeyCode::Char('/') | KeyCode::Char('i') => {
            app.input_mode = true;
        }
        KeyCode::Tab => {
            if app.nav.is_empty() {
                app.set_tab(app.tab.cycle());
            }
        }
        KeyCode::Char('1') => app.set_tab(Tab::Search),
        KeyCode::Char('2') => app.set_tab(Tab::Home),
        KeyCode::Char('3') => app.set_tab(Tab::Mixes),
        KeyCode::Char('4') => app.set_tab(Tab::Library),
        KeyCode::Char('5') => app.set_tab(Tab::Playlists),
        KeyCode::Char('6') => app.set_tab(Tab::Favorites),
        KeyCode::Char('7') => app.set_tab(Tab::Queue),
        KeyCode::Char('b') | KeyCode::Char('\\') => app.toggle_sidebar(),
        KeyCode::Char('o') => app.cycle_sort(),
        KeyCode::Char('t') => {
            if app.tab == Tab::Search && app.nav.is_empty() {
                app.search_scope = app.search_scope.cycle();
                app.recompute_filter();
                app.select_first();
            }
        }
        KeyCode::Char('S') => {
            if app.nav.is_empty() && app.tab == Tab::Favorites {
                app.fav_section = app.fav_section.cycle();
                app.recompute_filter();
                app.select_first();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.now_playing_mode || app.focus == Focus::NpQueue {
                app.move_np_queue(1);
            } else {
                app.move_selection(1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.now_playing_mode || app.focus == Focus::NpQueue {
                app.move_np_queue(-1);
            } else {
                app.move_selection(-1);
            }
        }
        KeyCode::Enter => {
            if app.now_playing_mode || app.focus == Focus::NpQueue {
                app.activate_np_queue().await;
            } else {
                app.activate().await;
            }
        }
        KeyCode::Char(' ') => app.toggle_pause(),
        KeyCode::Char('n') => app.play_next().await,
        KeyCode::Char('p') => app.play_previous().await,
        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char(']') => app.volume_up(),
        KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Char('[') => app.volume_down(),
        KeyCode::Right => app.seek_by(10.0),
        KeyCode::Left => app.seek_by(-10.0),
        KeyCode::Char('s') => app.cycle_shuffle(),
        KeyCode::Char('r') => app.cycle_repeat(),
        KeyCode::Char('R') => app.start_radio().await,
        KeyCode::Char('l') => app.toggle_love().await,
        KeyCode::Char('a') => app.add_selected().await,
        KeyCode::Char('A') => app.add_all().await,
        KeyCode::Char('m') => app.toggle_now_playing_mode(),
        KeyCode::Char('e') => app.toggle_spectrum(),
        KeyCode::Char('E') => app.cycle_eq_theme(),
        KeyCode::Char('x') => app.stop(),
        KeyCode::Char('f') => {
            app.load_library().await;
        }
        _ => {}
    }
}

async fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    if app.screen != Screen::Browse {
        return;
    }
    if matches!(app.popup, Some(Popup::Context(_))) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(Hit::ContextItem(i)) = app.hits.at(mouse.column, mouse.row) {
                    if let Some(Popup::Context(menu)) = app.popup.as_mut() {
                        menu.cursor = i;
                    }
                    app.context_activate().await;
                } else {
                    app.close_popup();
                }
            }
            MouseEventKind::Down(MouseButton::Right) => app.close_popup(),
            _ => {}
        }
        return;
    }
    if app.popup_open() {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            app.close_popup();
        }
        return;
    }

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if app.hits.queue.is_some_and(|r| {
                r.contains(ratatui::layout::Position {
                    x: mouse.column,
                    y: mouse.row,
                })
            }) || app.now_playing_mode
            {
                app.move_np_queue(-1);
            } else {
                app.move_selection(-1);
            }
        }
        MouseEventKind::ScrollDown => {
            if app.hits.queue.is_some_and(|r| {
                r.contains(ratatui::layout::Position {
                    x: mouse.column,
                    y: mouse.row,
                })
            }) || app.now_playing_mode
            {
                app.move_np_queue(1);
            } else {
                app.move_selection(1);
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            let Some(hit) = app.hits.at(mouse.column, mouse.row) else {
                return;
            };
            let track = match hit {
                Hit::ListRow(i) => {
                    app.select_index(i);
                    app.selected_track()
                }
                Hit::QueueRow(i) => {
                    app.np_queue_state.select(Some(i));
                    app.player.queue().items().get(i).cloned()
                }
                Hit::UpNext => app.player.queue().peek_next().cloned(),
                _ => app
                    .selected_track()
                    .or_else(|| app.player.now_playing().cloned()),
            };
            if let Some(track) = track {
                app.open_context(mouse.column, mouse.row, track);
            }
        }
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
            let Some(hit) = app.hits.at(mouse.column, mouse.row) else {
                return;
            };
            match hit {
                Hit::Tab(tab) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.set_tab(tab);
                    }
                }
                Hit::Fav(sec) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.set_fav_section(sec);
                    }
                }
                Hit::SidebarToggle => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.toggle_sidebar();
                    }
                }
                Hit::Sort(key) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.set_sort(key);
                    }
                }
                Hit::AlbumRow(i) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.open_page_album(i).await;
                    }
                }
                Hit::Playlist(i) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        if let Some(p) = app.playlists.get(i).cloned() {
                            app.open_playlist(p.uuid, p.title).await;
                        }
                    }
                }
                Hit::ListRow(i) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        let already = app.list_state.selected() == Some(i);
                        app.select_index(i);
                        if already {
                            app.activate().await;
                        }
                    }
                }
                Hit::QueueRow(i) => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        let already = app.np_queue_state.selected() == Some(i);
                        app.np_queue_state.select(Some(i));
                        app.focus = Focus::NpQueue;
                        if already {
                            app.activate_np_queue().await;
                        }
                    }
                }
                Hit::UpNext => {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        app.play_next().await;
                    }
                }
                Hit::Progress => {
                    if let Some(ratio) = app.hits.progress_ratio(mouse.column) {
                        app.seek_ratio(ratio);
                    }
                }
                Hit::ContextItem(_) => {}
            }
        }
        _ => {}
    }
}
