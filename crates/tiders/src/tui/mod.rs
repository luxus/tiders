//! The interactive terminal UI.

mod app;
mod art;
mod theme;
mod ui;

pub use app::App;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use tiders_core::config::Config;

use app::{Popup, Screen, View};
use art::ArtManager;

/// Launch the TUI against the given config, restoring the terminal on exit.
pub async fn run(config: Config) -> Result<()> {
    // Detect terminal image capability BEFORE touching the alternate screen.
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

async fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        let maybe_event = tokio::task::block_in_place(|| -> Result<Option<Event>> {
            if event::poll(Duration::from_millis(100))? {
                Ok(Some(event::read()?))
            } else {
                Ok(None)
            }
        })?;

        if let Some(Event::Key(key)) = maybe_event {
            if key.kind == KeyEventKind::Press {
                handle_key(app, key).await;
            }
        }

        app.on_tick().await;

        if app.should_quit {
            break;
        }
    }
    Ok(())
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
            // Help / Detail: any of these dismiss.
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
            KeyCode::Esc => app.input_mode = false,
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('d') => app.open_detail().await,
        KeyCode::Char('Q') => app.open_quality(),
        KeyCode::Char('/') | KeyCode::Char('i') => {
            app.input.clear();
            app.input_mode = true;
        }
        KeyCode::Tab => {
            let next = match app.view {
                View::Search => View::Favorites,
                View::Favorites => View::Queue,
                View::Queue => View::Search,
            };
            app.set_view(next);
        }
        KeyCode::Char('1') => app.set_view(View::Search),
        KeyCode::Char('2') => app.set_view(View::Favorites),
        KeyCode::Char('3') => app.set_view(View::Queue),
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::Enter => app.play_selected().await,
        KeyCode::Char(' ') => app.toggle_pause(),
        KeyCode::Char('n') => app.play_next().await,
        KeyCode::Char('p') => app.play_previous().await,
        KeyCode::Char('+') | KeyCode::Char('=') => app.volume_up(),
        KeyCode::Char('-') | KeyCode::Char('_') => app.volume_down(),
        KeyCode::Char('s') => app.stop(),
        KeyCode::Char('f') => {
            app.load_favorites().await;
            app.set_view(View::Favorites);
        }
        _ => {}
    }
}
