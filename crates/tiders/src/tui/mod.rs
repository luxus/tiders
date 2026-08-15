//! The interactive terminal UI.

mod anim;
mod app;
mod art;
mod theme;
mod ui;

pub use app::App;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

use tiders_core::config::Config;

use app::{Popup, Screen, Tab, FRAME};
use art::ArtManager;

/// Launch the TUI against the given config, restoring the terminal on exit.
pub async fn run(config: Config) -> Result<()> {
    let art = ArtManager::new();
    let mut app = App::bootstrap(config, art).await?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// Dedicated OS thread for input — grok-build's workaround for crossterm
/// stranding `EventStream` wakers when the future is dropped inside `select!`.
fn spawn_input_thread(tx: mpsc::UnboundedSender<KeyEvent>, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("tiders-input".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match crossterm::event::poll(Duration::from_millis(16)) {
                    Ok(true) => match crossterm::event::read() {
                        Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                            if tx.send(key).is_err() {
                                break;
                            }
                        }
                        Ok(_) => {}
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

    let mut frames = interval(FRAME);
    frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
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

        tokio::select! {
            biased;
            maybe = key_rx.recv() => {
                match maybe {
                    Some(key) => {
                        handle_key(app, key).await;
                        dirty = true;
                    }
                    None => break Ok(()),
                }
            }
            _ = frames.tick() => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last).as_secs_f32();
                last = now;
                app.on_frame(dt).await;
                if app.needs_frames() || last_idle_draw.elapsed() >= Duration::from_millis(250) {
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
                handle_popup_key(app, key);
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

fn handle_popup_key(app: &mut App, key: KeyEvent) {
    match &app.popup {
        Some(Popup::Quality) => match key.code {
            KeyCode::Up | KeyCode::Char('k') => app.quality_move(-1),
            KeyCode::Down | KeyCode::Char('j') => app.quality_move(1),
            KeyCode::Enter => app.apply_quality(),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => app.close_popup(),
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
            KeyCode::Enter => app.submit_search().await,
            KeyCode::Esc => {
                app.input_mode = false;
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            if !app.go_back() {
                app.quit();
            }
        }
        KeyCode::Char('q') => app.quit(),
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('d') => app.open_detail().await,
        KeyCode::Char('Q') => app.open_quality(),
        KeyCode::Char('/') | KeyCode::Char('i') => {
            app.input.clear();
            app.input_mode = true;
        }
        KeyCode::Tab => {
            if app.nav.is_empty() {
                app.set_tab(app.tab.cycle());
            }
        }
        KeyCode::Char('1') => app.set_tab(Tab::Search),
        KeyCode::Char('2') => app.set_tab(Tab::Library),
        KeyCode::Char('3') => app.set_tab(Tab::Favorites),
        KeyCode::Char('4') => app.set_tab(Tab::Queue),
        KeyCode::Char('t') => {
            if app.tab == Tab::Search && app.nav.is_empty() {
                app.search_scope = app.search_scope.cycle();
                app.select_first();
            }
        }
        KeyCode::Char('S') => {
            if app.nav.is_empty() {
                match app.tab {
                    Tab::Library => {
                        app.lib_section = app.lib_section.cycle();
                        app.select_first();
                    }
                    Tab::Favorites => {
                        app.fav_section = app.fav_section.cycle();
                        app.select_first();
                    }
                    _ => {}
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::Enter => app.activate().await,
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
            app.set_tab(Tab::Favorites);
        }
        _ => {}
    }
}