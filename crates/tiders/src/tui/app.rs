//! TUI application state and the async update logic.

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::widgets::ListState;
use ratatui_image::protocol::StatefulProtocol;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use tiders_core::config::{Config, Quality, Settings};
use tiders_core::model::{SearchResults, TrackView};
use tiders_core::tidlers::client::oauth::OAuthStatus;
use tiders_core::{DeviceLogin, Player, PlayerStatus, TidalService};

use super::art::ArtManager;

/// Frames the popup open-animation plays over.
pub const POPUP_OPEN_FRAMES: u8 = 6;
/// How many ticks a toast stays on screen (~120ms each).
pub const TOAST_TICKS: u64 = 26;
/// Selectable qualities, in the quality popup's order.
pub const QUALITIES: [Quality; 4] = [
    Quality::Low,
    Quality::High,
    Quality::Lossless,
    Quality::HiRes,
];

/// Which top-level screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
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

/// A modal overlay.
pub enum Popup {
    Help,
    Detail(TrackView),
    Quality,
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

    // Album art.
    pub art: ArtManager,
    pub now_art: Option<StatefulProtocol>,
    pub detail_art: Option<StatefulProtocol>,

    // Modals + animation.
    pub popup: Option<Popup>,
    pub popup_anim: u8,
    pub quality_cursor: usize,

    // Playback progress (approximated from wall-clock while playing).
    play_started: Option<Instant>,
    play_base: Duration,

    pub status: String,
    pub toast: Option<String>,
    toast_tick: u64,
    pub loading: bool,

    pub login: Option<LoginState>,
    pub tick: u64,
    pub should_quit: bool,
}

impl App {
    /// Build the app: restore a session if possible, otherwise start a login.
    pub async fn bootstrap(config: Config, art: ArtManager) -> Result<Self> {
        let settings = config.load_settings().unwrap_or_default();
        let player = Player::with_backend(settings.backend, settings.volume)?;
        let quality_cursor = QUALITIES
            .iter()
            .position(|q| *q == settings.quality)
            .unwrap_or(2);

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
            art,
            now_art: None,
            detail_art: None,
            popup: None,
            popup_anim: 0,
            quality_cursor,
            play_started: None,
            play_base: Duration::ZERO,
            status: String::new(),
            toast: None,
            toast_tick: 0,
            loading: false,
            login: None,
            tick: 0,
            should_quit: false,
        };

        match TidalService::restore(config.clone(), settings.quality).await {
            Ok(Some(service)) => {
                let who = service.username().unwrap_or_else(|| "your account".into());
                app.service = Some(service);
                app.screen = Screen::Browse;
                app.set_status(format!("Signed in as {who}"));
                app.load_favorites().await;
                app.view = if app.favorites.is_empty() {
                    View::Search
                } else {
                    View::Favorites
                };
                app.select_first();
            }
            Ok(None) => app.start_login(),
            Err(e) => {
                app.set_status(format!("Could not restore session: {e}"));
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

        // Popup open animation.
        if self.popup.is_some() && self.popup_anim < POPUP_OPEN_FRAMES {
            self.popup_anim += 1;
        }

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
                self.clear_timing();
                self.now_art = None;
                self.set_status("Queue finished".into());
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
                self.set_status(format!("Signed in as {who}"));
                self.load_favorites().await;
                self.view = if self.favorites.is_empty() {
                    View::Search
                } else {
                    View::Favorites
                };
                self.select_first();
            }
            Ok(None) | Err(_) => {
                self.set_status("Login completed but no session was saved — try again".into());
                self.start_login();
            }
        }
    }

    // ── data loading ────────────────────────────────────────────────────

    pub async fn load_favorites(&mut self) {
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        // The `self.service` borrow lasts only for this awaited expression.
        let result = self
            .service
            .as_ref()
            .expect("service present")
            .favorite_tracks(100, 0)
            .await;
        self.loading = false;
        match result {
            Ok(tracks) => {
                let n = tracks.len();
                self.favorites = tracks;
                self.set_status(format!("Loaded {n} favorite track(s)"));
            }
            Err(e) => self.set_status(format!("Failed to load favorites: {e}")),
        }
    }

    pub async fn submit_search(&mut self) {
        let query = self.input.trim().to_string();
        self.input_mode = false;
        if query.is_empty() {
            return;
        }
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        self.set_status(format!("Searching for “{query}”…"));
        let result = self
            .service
            .as_ref()
            .expect("service present")
            .search(&query, 25)
            .await;
        self.loading = false;
        match result {
            Ok(results) => {
                self.set_status(format!("{} result(s) for “{query}”", results.tracks.len()));
                self.results = results;
                self.view = View::Search;
                self.select_first();
            }
            Err(e) => self.set_status(format!("Search failed: {e}")),
        }
    }

    // ── playback ────────────────────────────────────────────────────────

    pub async fn play_selected(&mut self) {
        let (tracks, idx) = self.current_tracks_with_selection();
        if tracks.is_empty() {
            return;
        }
        self.player.set_queue(tracks, idx);
        self.play_current().await;
    }

    async fn play_current(&mut self) {
        let Some(track) = self.player.current().cloned() else {
            return;
        };
        let quality = self.settings.quality;
        let Some(service) = self.service.as_mut() else {
            return;
        };
        // Bind the awaited result so the `service` borrow of `self` ends here,
        // freeing `self.player`/`self.art` for the rest of the function.
        let stream = service.stream_url(track.id, quality).await;
        match stream {
            Ok(info) => match self.player.play_current(&info.url) {
                Ok(()) => {
                    self.play_base = Duration::ZERO;
                    self.play_started = Some(Instant::now());
                    self.set_status(format!("▶ {}", track.label()));
                    self.load_now_art(track.cover.clone()).await;
                }
                Err(e) => self.set_status(format!("Playback error: {e}")),
            },
            Err(e) => self.set_status(format!("Could not get stream: {e}")),
        }
    }

    async fn load_now_art(&mut self, cover: Option<String>) {
        self.now_art = match cover {
            Some(id) => self.art.protocol_for(&id).await,
            None => None,
        };
    }

    pub async fn play_next(&mut self) {
        if self.player.next_track().is_some() {
            self.play_current().await;
        }
    }

    pub async fn play_previous(&mut self) {
        if self.player.previous_track().is_some() {
            self.play_current().await;
        }
    }

    pub fn toggle_pause(&mut self) {
        match self.player.status() {
            PlayerStatus::Playing => {
                self.pause_timing();
                let _ = self.player.pause();
                self.set_status("Paused".into());
            }
            PlayerStatus::Paused => {
                self.play_started = Some(Instant::now());
                let _ = self.player.resume();
                self.set_status("Resumed".into());
            }
            PlayerStatus::Stopped => {}
        }
    }

    pub fn stop(&mut self) {
        let _ = self.player.stop();
        self.clear_timing();
        self.now_art = None;
        self.set_status("Stopped".into());
    }

    pub fn volume_up(&mut self) {
        let _ = self.player.volume_up(5);
        self.set_status(format!("Volume {}%", self.player.volume()));
    }

    pub fn volume_down(&mut self) {
        let _ = self.player.volume_down(5);
        self.set_status(format!("Volume {}%", self.player.volume()));
    }

    fn pause_timing(&mut self) {
        if let Some(start) = self.play_started.take() {
            self.play_base += start.elapsed();
        }
    }

    fn clear_timing(&mut self) {
        self.play_started = None;
        self.play_base = Duration::ZERO;
    }

    /// Elapsed playback time (approximate; wall-clock while playing).
    pub fn elapsed_secs(&self) -> u64 {
        let mut e = self.play_base;
        if let Some(start) = self.play_started {
            e += start.elapsed();
        }
        e.as_secs()
    }

    /// Playback progress in `0.0..=1.0` for the current track.
    pub fn progress(&self) -> f64 {
        let dur = self
            .player
            .now_playing()
            .map(|t| t.duration_secs)
            .unwrap_or(0);
        if dur == 0 {
            return 0.0;
        }
        (self.elapsed_secs() as f64 / dur as f64).clamp(0.0, 1.0)
    }

    // ── popups ──────────────────────────────────────────────────────────

    pub fn popup_open(&self) -> bool {
        self.popup.is_some()
    }

    fn open_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
        self.popup_anim = 0;
    }

    pub fn toggle_help(&mut self) {
        if matches!(self.popup, Some(Popup::Help)) {
            self.close_popup();
        } else {
            self.open_popup(Popup::Help);
        }
    }

    pub fn open_quality(&mut self) {
        self.quality_cursor = QUALITIES
            .iter()
            .position(|q| *q == self.settings.quality)
            .unwrap_or(2);
        self.open_popup(Popup::Quality);
    }

    /// Open the detail popup for the highlighted track, loading its cover.
    pub async fn open_detail(&mut self) {
        let (tracks, idx) = self.current_tracks_with_selection();
        let Some(track) = tracks.get(idx).cloned() else {
            return;
        };
        self.detail_art = match track.cover.clone() {
            Some(id) => self.art.protocol_for(&id).await,
            None => None,
        };
        self.open_popup(Popup::Detail(track));
    }

    pub fn close_popup(&mut self) {
        self.popup = None;
        self.popup_anim = 0;
        self.detail_art = None;
    }

    pub fn quality_move(&mut self, delta: isize) {
        let n = QUALITIES.len() as isize;
        self.quality_cursor = (self.quality_cursor as isize + delta).rem_euclid(n) as usize;
    }

    pub fn apply_quality(&mut self) {
        if let Some(q) = QUALITIES.get(self.quality_cursor).copied() {
            self.settings.quality = q;
            let _ = self.config.save_settings(&self.settings);
            self.set_status(format!("Quality set to {}", q.label()));
        }
        self.close_popup();
    }

    // ── status / toast ──────────────────────────────────────────────────

    pub fn set_status(&mut self, msg: String) {
        self.toast = Some(msg.clone());
        self.toast_tick = self.tick;
        self.status = msg;
    }

    /// Toast fade factor in `0.0..=1.0`, or `None` once it has expired.
    pub fn toast_alpha(&self) -> Option<f32> {
        let age = self.tick.saturating_sub(self.toast_tick);
        if age >= TOAST_TICKS {
            return None;
        }
        // Full brightness for the first third, then fade out.
        let third = TOAST_TICKS / 3;
        if age <= third {
            Some(1.0)
        } else {
            let span = (TOAST_TICKS - third) as f32;
            Some((1.0 - (age - third) as f32 / span).clamp(0.0, 1.0))
        }
    }

    // ── selection helpers ───────────────────────────────────────────────

    pub fn current_tracks_with_selection(&self) -> (Vec<TrackView>, usize) {
        let tracks = self.current_tracks();
        let idx = self
            .list_state
            .selected()
            .unwrap_or(0)
            .min(tracks.len().saturating_sub(1));
        (tracks, idx)
    }

    pub fn current_tracks(&self) -> Vec<TrackView> {
        match self.view {
            View::Search => self.results.tracks.clone(),
            View::Favorites => self.favorites.clone(),
            View::Queue => self.player.queue().items().to_vec(),
        }
    }

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

    pub fn open_login_url(&mut self) {
        if let Some(login) = self.login.as_ref() {
            if let Some(code) = login.code.as_ref() {
                let _ = open::that(code.url());
                self.set_status("Opened TIDAL login in your browser".into());
            }
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }
}

/// Spawn the background login task and return its live [`LoginState`].
fn spawn_login(config: Config, quality: Quality) -> LoginState {
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
