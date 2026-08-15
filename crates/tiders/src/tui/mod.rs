//! The interactive terminal UI.

mod app;
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

use app::{Screen, View};

/// Launch the TUI against the given config, restoring the terminal on exit.
pub async fn run(config: Config) -> Result<()> {
    let mut app = App::bootstrap(config).await?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut app).await;

    // Always restore the terminal, even if the loop errored.
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

        // Poll for an input event with a short timeout so the UI keeps ticking
        // (spinner animation, login progress, queue auto-advance).
        let maybe_event = tokio::task::block_in_place(|| -> Result<Option<Event>> {
            if event::poll(Duration::from_millis(120))? {
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
    // Ctrl-C always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit();
        return;
    }

    match app.screen {
        Screen::Login => handle_login_key(app, key),
        Screen::Browse => handle_browse_key(app, key).await,
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
