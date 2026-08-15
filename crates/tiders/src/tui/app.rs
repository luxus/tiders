//! TUI application state and the async update logic.

use anyhow::Result;
use ratatui::widgets::ListState;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use tiders_core::config::{Config, Settings};
use tiders_core::model::{SearchResults, TrackView};
use tiders_core::tidlers::client::oauth::OAuthStatus;
use tiders_core::{DeviceLogin, Player, TidalService};

/// Which top-level screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Signing in via the device-code flow.
    Login,
    /// Browsing and playing.
    Browse,
}

/// Which content view is active in [`Screen::Browse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Search,
    Favorites,
    Queue,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Search => "Search",
            View::Favorites => "Favorites",
            View::Queue => "Queue",
        }
    }
}

/// A message from the background login task.
#[derive(Debug)]
enum LoginEvent {
    Code(DeviceLogin),
    Status(String),
    Done,
    Failed(String),
}

/// State for the in-progress device-code login.
pub struct LoginState {
    rx: UnboundedReceiver<LoginEvent>,
    pub code: Option<DeviceLogin>,
    pub status: String,
    pub error: Option<String>,
}

/// The whole TUI application.
pub struct App {
    pub config: Config,
    pub settings: Settings,
    pub service: Option<TidalService>,
    pub player: Player,

    pub screen: Screen,
    pub view: View,

    pub input_mode: bool,
    pub input: String,

    pub results: SearchResults,
    pub favorites: Vec<TrackView>,
    pub list_state: ListState,

    pub status: String,
    pub login: Option<LoginState>,
    pub tick: u64,
    pub should_quit: bool,
}

impl App {
    /// Build the app: restore a session if possible, otherwise start a login.
    pub async fn bootstrap(config: Config) -> Result<Self> {
        let settings = config.load_settings().unwrap_or_default();
        let player = Player::with_backend(settings.backend, settings.volume)?;

        let mut app = App {
            config: config.clone(),
            settings: settings.clone(),
            service: None,
            player,
            screen: Screen::Login,
            view: View::Search,
            input_mode: false,
            input: String::new(),
            results: SearchResults::default(),
            favorites: Vec::new(),
            list_state: ListState::default(),
            status: String::new(),
            login: None,
            tick: 0,
            should_quit: false,
        };

        match TidalService::restore(config.clone(), settings.quality).await {
            Ok(Some(service)) => {
                let who = service.username().unwrap_or_else(|| "your account".into());
                app.service = Some(service);
                app.screen = Screen::Browse;
                app.status = format!(
                    "Signed in as {who} · backend: {}",
                    app.player.backend_name()
                );
                app.load_favorites().await;
                app.view = if app.favorites.is_empty() {
                    View::Search
                } else {
                    View::Favorites
                };
                app.select_first();
            }
            Ok(None) => {
                app.start_login();
            }
            Err(e) => {
                app.status = format!("Could not restore session: {e}");
                app.start_login();
            }
        }

        Ok(app)
    }

    /// Kick off a background device-code login.
    pub fn start_login(&mut self) {
        self.screen = Screen::Login;
        self.login = Some(spawn_login(self.config.clone(), self.settings.quality));
    }

    /// Drain background login events and advance timed state.
    pub async fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        // Login progress.
        let mut finished = false;
        if let Some(login) = self.login.as_mut() {
            while let Ok(event) = login.rx.try_recv() {
                match event {
                    LoginEvent::Code(code) => {
                        login.status = "Waiting for you to authorize in the browser…".into();
                        login.code = Some(code);
                    }
                    LoginEvent::Status(s) => login.status = s,
                    LoginEvent::Done => finished = true,
                    LoginEvent::Failed(e) => login.error = Some(e),
                }
            }
        }
        if finished {
            self.finish_login().await;
        }

        // Auto-advance the queue when a track ends.
        if self.screen == Screen::Browse && self.player.poll_finished() {
            if self.player.next_track().is_some() {
                self.play_current().await;
            } else {
                self.status = "Queue finished".into();
            }
        }
    }

    async fn finish_login(&mut self) {
        self.login = None;
        match TidalService::restore(self.config.clone(), self.settings.quality).await {
            Ok(Some(service)) => {
                let who = service.username().unwrap_or_else(|| "your account".into());
                self.service = Some(service);
                self.screen = Screen::Browse;
                self.status = format!(
                    "Signed in as {who} · backend: {}",
                    self.player.backend_name()
                );
                self.load_favorites().await;
                self.view = if self.favorites.is_empty() {
                    View::Search
                } else {
                    View::Favorites
                };
                self.select_first();
            }
            Ok(None) | Err(_) => {
                self.status = "Login completed but no session was saved — try again".into();
                self.start_login();
            }
        }
    }

    // ── data loading ────────────────────────────────────────────────────

    /// Load the user's favorite tracks.
    pub async fn load_favorites(&mut self) {
        if let Some(service) = self.service.as_ref() {
            match service.favorite_tracks(100, 0).await {
                Ok(tracks) => {
                    let n = tracks.len();
                    self.favorites = tracks;
                    self.status = format!("Loaded {n} favorite track(s)");
                }
                Err(e) => self.status = format!("Failed to load favorites: {e}"),
            }
        }
    }

    /// Run the current search query.
    pub async fn submit_search(&mut self) {
        let query = self.input.trim().to_string();
        self.input_mode = false;
        if query.is_empty() {
            return;
        }
        if let Some(service) = self.service.as_ref() {
            self.status = format!("Searching for “{query}”…");
            match service.search(&query, 25).await {
                Ok(results) => {
                    self.status = format!("{} result(s) for “{query}”", results.tracks.len());
                    self.results = results;
                    self.view = View::Search;
                    self.select_first();
                }
                Err(e) => self.status = format!("Search failed: {e}"),
            }
        }
    }

    // ── playback ────────────────────────────────────────────────────────

    /// Play the currently highlighted track, seeding the queue from its list.
    pub async fn play_selected(&mut self) {
        let (tracks, idx) = self.current_tracks_with_selection();
        if tracks.is_empty() {
            return;
        }
        self.player.set_queue(tracks, idx);
        self.play_current().await;
    }

    /// Resolve a stream URL for the queue's current track and start playback.
    async fn play_current(&mut self) {
        let Some(track) = self.player.current().cloned() else {
            return;
        };
        let quality = self.settings.quality;
        let Some(service) = self.service.as_mut() else {
            return;
        };
        match service.stream_url(track.id, quality).await {
            Ok(info) => match self.player.play_current(&info.url) {
                Ok(()) => {
                    self.status = format!(
                        "▶ {} · {} · {}",
                        track.label(),
                        quality.label(),
                        self.player.backend_name()
                    );
                }
                Err(e) => self.status = format!("Playback error: {e}"),
            },
            Err(e) => self.status = format!("Could not get stream: {e}"),
        }
    }

    /// Skip to the next queued track.
    pub async fn play_next(&mut self) {
        if self.player.next_track().is_some() {
            self.play_current().await;
        }
    }

    /// Return to the previous queued track.
    pub async fn play_previous(&mut self) {
        if self.player.previous_track().is_some() {
            self.play_current().await;
        }
    }

    /// Toggle pause/resume.
    pub fn toggle_pause(&mut self) {
        let _ = self.player.toggle_pause();
    }

    /// Stop playback.
    pub fn stop(&mut self) {
        let _ = self.player.stop();
        self.status = "Stopped".into();
    }

    /// Nudge the volume up.
    pub fn volume_up(&mut self) {
        let _ = self.player.volume_up(5);
        self.status = format!("Volume {}%", self.player.volume());
    }

    /// Nudge the volume down.
    pub fn volume_down(&mut self) {
        let _ = self.player.volume_down(5);
        self.status = format!("Volume {}%", self.player.volume());
    }

    // ── selection helpers ───────────────────────────────────────────────

    /// The tracks shown in the current view, plus the selected index.
    pub fn current_tracks_with_selection(&self) -> (Vec<TrackView>, usize) {
        let tracks = self.current_tracks();
        let idx = self
            .list_state
            .selected()
            .unwrap_or(0)
            .min(tracks.len().saturating_sub(1));
        (tracks, idx)
    }

    /// The tracks shown in the current view.
    pub fn current_tracks(&self) -> Vec<TrackView> {
        match self.view {
            View::Search => self.results.tracks.clone(),
            View::Favorites => self.favorites.clone(),
            View::Queue => self.player.queue().items().to_vec(),
        }
    }

    /// Number of rows in the current view.
    pub fn current_len(&self) -> usize {
        match self.view {
            View::Search => self.results.tracks.len(),
            View::Favorites => self.favorites.len(),
            View::Queue => self.player.queue().len(),
        }
    }

    pub fn set_view(&mut self, view: View) {
        self.view = view;
        self.select_first();
    }

    pub fn select_first(&mut self) {
        if self.current_len() > 0 {
            self.list_state.select(Some(0));
        } else {
            self.list_state.select(None);
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.current_len();
        if len == 0 {
            self.list_state.select(None);
            return;
        }
        let current = self.list_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.list_state.select(Some(next));
    }

    /// Open the login URL in the system browser (best effort).
    pub fn open_login_url(&mut self) {
        if let Some(login) = self.login.as_ref() {
            if let Some(code) = login.code.as_ref() {
                let _ = open::that(code.url());
                self.status = "Opened TIDAL login in your browser".into();
            }
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }
}

/// Spawn the background login task and return its live [`LoginState`].
fn spawn_login(config: Config, quality: tiders_core::config::Quality) -> LoginState {
    let (tx, rx) = unbounded_channel();

    tokio::spawn(async move {
        let mut service = TidalService::new(config, quality);
        let device = match service.begin_device_login().await {
            Ok(device) => device,
            Err(e) => {
                let _ = tx.send(LoginEvent::Failed(e.to_string()));
                return;
            }
        };
        let _ = tx.send(LoginEvent::Code(device.clone()));

        // Forward OAuth status updates to the UI.
        let (status_tx, mut status_rx) = unbounded_channel::<OAuthStatus>();
        let tx_status = tx.clone();
        tokio::spawn(async move {
            while let Some(status) = status_rx.recv().await {
                let msg = match status {
                    OAuthStatus::Waiting => "Waiting for authorization…".to_string(),
                    OAuthStatus::Success => "Authorized!".to_string(),
                    OAuthStatus::Error(e) => format!("Error: {e}"),
                };
                let _ = tx_status.send(LoginEvent::Status(msg));
            }
        });

        match service
            .complete_device_login(&device, Some(status_tx))
            .await
        {
            Ok(()) => {
                let _ = tx.send(LoginEvent::Done);
            }
            Err(e) => {
                let _ = tx.send(LoginEvent::Failed(e.to_string()));
            }
        }
    });

    LoginState {
        rx,
        code: None,
        status: "Requesting a device code from TIDAL…".into(),
        error: None,
    }
}
