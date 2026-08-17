//! TUI application state and the async update logic.

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::widgets::ListState;
use ratatui_image::protocol::StatefulProtocol;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use tiders_core::config::{Config, Quality, Settings};
use tiders_core::lyrics::LyricLine;
use tiders_core::media::{MediaBridge, MediaCommand, MediaNowPlaying};
use tiders_core::model::{
    AlbumView, ArtistView, HomeCard, HomeCardKind, MixView, PlaylistView, SearchResults,
    StreamQuality, TrackView,
};
use tiders_core::playcount::PlayCountStore;
use tiders_core::queue::{RepeatMode, ShuffleMode};
use tiders_core::spectrum::Spectrum;
use tiders_core::tidlers::client::oauth::OAuthStatus;
use tiders_core::{DeviceLogin, Player, PlayerStatus, TidalService};

use super::art::ArtManager;

/// Target presentation rate (matches grok-build-style 120 Hz TUIs).
pub const FRAME_HZ: u32 = 120;
pub const FRAME: Duration = Duration::from_nanos(1_000_000_000 / FRAME_HZ as u64);
pub const POPUP_OPEN_SECS: f32 = 0.18;
pub const TOAST_SECS: f32 = 2.6;

pub const QUALITIES: [Quality; 4] = [
    Quality::Low,
    Quality::High,
    Quality::Lossless,
    Quality::HiRes,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Browse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Home,
    Mixes,
    Library,
    Playlists,
    Favorites,
    Queue,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Search => "Search",
            Tab::Home => "For You",
            Tab::Mixes => "Mixes",
            Tab::Library => "Library",
            Tab::Playlists => "Playlists",
            Tab::Favorites => "Favorites",
            Tab::Queue => "Queue",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Tab::Search => Tab::Home,
            Tab::Home => Tab::Mixes,
            Tab::Mixes => Tab::Library,
            Tab::Library => Tab::Playlists,
            Tab::Playlists => Tab::Favorites,
            Tab::Favorites => Tab::Queue,
            Tab::Queue => Tab::Search,
        }
    }

    pub fn all() -> [Tab; 7] {
        [
            Tab::Search,
            Tab::Home,
            Tab::Mixes,
            Tab::Library,
            Tab::Playlists,
            Tab::Favorites,
            Tab::Queue,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Title,
    Artist,
    Album,
    Time,
}

impl SortKey {
    pub fn title(self) -> &'static str {
        match self {
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Album => "Album",
            SortKey::Time => "Time",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            SortKey::Title => SortKey::Artist,
            SortKey::Artist => SortKey::Album,
            SortKey::Album => SortKey::Time,
            SortKey::Time => SortKey::Title,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    GoArtist,
    GoAlbum,
    ToggleFav,
    Like,
    Dislike,
}

impl ContextAction {
    pub fn all() -> [ContextAction; 5] {
        [
            ContextAction::GoArtist,
            ContextAction::GoAlbum,
            ContextAction::ToggleFav,
            ContextAction::Like,
            ContextAction::Dislike,
        ]
    }
}

pub struct ContextMenu {
    pub x: u16,
    pub y: u16,
    pub cursor: usize,
    pub track: TrackView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Main,
    Sidebar,
    NpQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FavSection {
    Tracks,
    Albums,
    Artists,
}

impl FavSection {
    pub fn title(self) -> &'static str {
        match self {
            FavSection::Tracks => "Tracks",
            FavSection::Albums => "Albums",
            FavSection::Artists => "Artists",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            FavSection::Tracks => FavSection::Albums,
            FavSection::Albums => FavSection::Artists,
            FavSection::Artists => FavSection::Tracks,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl SearchScope {
    pub fn title(self) -> &'static str {
        match self {
            SearchScope::Tracks => "Tracks",
            SearchScope::Albums => "Albums",
            SearchScope::Artists => "Artists",
            SearchScope::Playlists => "Playlists",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            SearchScope::Tracks => SearchScope::Albums,
            SearchScope::Albums => SearchScope::Artists,
            SearchScope::Artists => SearchScope::Playlists,
            SearchScope::Playlists => SearchScope::Tracks,
        }
    }
}

/// Drill-down page sitting on top of a tab.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Page {
    Playlist { uuid: String, title: String },
    Mix { id: String, title: String },
    Album { id: u64, title: String },
    Artist { id: u64, name: String },
}

pub enum Popup {
    Help,
    Detail(Box<TrackView>),
    Quality,
    Context(Box<ContextMenu>),
}

#[derive(Debug)]
enum LoginEvent {
    Code(DeviceLogin),
    Status(String),
    Done,
    Failed(String),
}

pub struct LoginState {
    rx: UnboundedReceiver<LoginEvent>,
    pub code: Option<DeviceLogin>,
    pub status: String,
    pub error: Option<String>,
}

pub struct Toast {
    pub title: String,
    pub subtitle: Option<String>,
    pub cover: Option<String>,
    pub art: Option<ratatui_image::protocol::StatefulProtocol>,
    pub started: Instant,
}

pub struct App {
    pub config: Config,
    pub settings: Settings,
    pub service: Option<TidalService>,
    pub player: Player,
    pub counts: PlayCountStore,
    pub media: MediaBridge,
    pub spectrum: Spectrum,

    pub screen: Screen,
    pub tab: Tab,
    pub fav_section: FavSection,
    pub search_scope: SearchScope,
    pub nav: Vec<Page>,
    pub focus: Focus,
    pub sort_key: SortKey,
    pub sort_asc: bool,

    pub input_mode: bool,
    pub input: String,
    /// Ranked hits for the current `input` needle (FFF / neo_frizbee).
    /// `None` means the needle is empty and the view is unfiltered.
    filter_hits: Option<Vec<super::filter::Hit>>,
    /// Source-row index → position in `filter_hits` so highlight lookup stays
    /// O(1) after the visible view is sorted.
    filter_by_src: std::collections::HashMap<usize, usize>,

    pub results: SearchResults,
    pub favorites: Vec<TrackView>,
    pub fav_albums: Vec<AlbumView>,
    pub fav_artists: Vec<ArtistView>,
    pub playlists: Vec<PlaylistView>,
    pub mixes: Vec<MixView>,
    pub for_you: Vec<HomeCard>,
    pub page_tracks: Vec<TrackView>,
    pub page_albums: Vec<AlbumView>,
    pub page_bio: String,
    pub page_artist: Option<ArtistView>,
    pub page_art: Option<StatefulProtocol>,
    pub loved: std::collections::HashSet<u64>,
    pub home_error: Option<String>,

    pub list_state: ListState,
    pub np_queue_state: ListState,

    pub art: ArtManager,
    pub now_art: Option<StatefulProtocol>,
    pub detail_art: Option<StatefulProtocol>,

    pub popup: Option<Popup>,
    pub popup_opened: Option<Instant>,
    pub quality_cursor: usize,

    pub now_playing_mode: bool,
    pub np_progress: f32,

    pub lyrics: Vec<LyricLine>,
    pub stream_quality: StreamQuality,

    pub status: String,
    pub toast: Option<Toast>,
    pub loading: bool,

    pub login: Option<LoginState>,
    pub tick: u64,
    pub should_quit: bool,
    last_media: Instant,
    pub hits: super::hits::HitMap,
    /// Local `file://` cover for macOS Now Playing (HTTP URLs are ignored).
    media_cover_url: Option<String>,
    media_cover_path: Option<std::path::PathBuf>,
}

impl App {
    pub async fn bootstrap(config: Config, art: ArtManager) -> Result<Self> {
        let settings = config.load_settings().unwrap_or_default();
        let player = Player::with_backend_replaygain(
            settings.backend,
            settings.volume,
            settings.replaygain.mpv_flag(),
        )?;
        let counts = PlayCountStore::load_or_empty(config.playcounts_path());
        let quality_cursor = QUALITIES
            .iter()
            .position(|q| *q == settings.quality)
            .unwrap_or(2);

        let mut app = App {
            config: config.clone(),
            settings: settings.clone(),
            service: None,
            player,
            counts,
            media: MediaBridge::start(),
            spectrum: Spectrum::new(48),
            screen: Screen::Login,
            tab: Tab::Search,
            fav_section: FavSection::Tracks,
            search_scope: SearchScope::Tracks,
            nav: Vec::new(),
            focus: Focus::Main,
            sort_key: SortKey::Title,
            sort_asc: true,
            input_mode: false,
            input: String::new(),
            filter_hits: None,
            filter_by_src: std::collections::HashMap::new(),
            results: SearchResults::default(),
            favorites: Vec::new(),
            fav_albums: Vec::new(),
            fav_artists: Vec::new(),
            playlists: Vec::new(),
            mixes: Vec::new(),
            for_you: Vec::new(),
            page_tracks: Vec::new(),
            page_albums: Vec::new(),
            page_bio: String::new(),
            page_artist: None,
            page_art: None,
            loved: std::collections::HashSet::new(),
            home_error: None,
            list_state: ListState::default(),
            np_queue_state: ListState::default(),
            art,
            now_art: None,
            detail_art: None,
            popup: None,
            popup_opened: None,
            quality_cursor,
            now_playing_mode: false,
            np_progress: 0.0,
            lyrics: Vec::new(),
            stream_quality: StreamQuality::default(),
            status: String::new(),
            toast: None,
            loading: false,
            login: None,
            tick: 0,
            should_quit: false,
            last_media: Instant::now(),
            hits: super::hits::HitMap::default(),
            media_cover_url: None,
            media_cover_path: None,
        };

        app.player.set_shuffle(settings.shuffle, Some(&app.counts));
        app.player.set_repeat(settings.repeat);
        app.restore_queue();

        match TidalService::restore(config.clone(), settings.quality).await {
            Ok(Some(service)) => {
                let who = service.username().unwrap_or_else(|| "your account".into());
                app.service = Some(service);
                app.screen = Screen::Browse;
                app.set_status(format!("Signed in as {who}"));
                app.load_library().await;
                app.tab = if app.favorites.is_empty() {
                    Tab::Search
                } else {
                    Tab::Favorites
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

    pub fn start_login(&mut self) {
        self.screen = Screen::Login;
        self.login = Some(spawn_login(self.config.clone(), self.settings.quality));
    }

    pub async fn on_frame(&mut self, dt: f32) {
        self.tick = self.tick.wrapping_add(1);

        let target = if self.now_playing_mode { 1.0 } else { 0.0 };
        self.np_progress = super::anim::damp(self.np_progress, target, dt, 14.0);

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

        for cmd in self.media.poll() {
            self.handle_media(cmd).await;
        }

        if self.screen == Screen::Browse {
            if self.player.poll_finished() {
                if self.player.next_track().is_some() {
                    self.play_current().await;
                } else {
                    self.now_art = None;
                    self.lyrics.clear();
                    self.set_status("Queue finished".into());
                }
            }
            self.player.note_progress(dt as f64, Some(&mut self.counts));
            if self.tick % 8 == 0 {
                self.player.refresh_quality();
                self.stream_quality = self.player.quality().clone();
            }
        }

        let playing = self.player.status() == PlayerStatus::Playing;
        if self.settings.show_spectrum {
            let bins = self.player.drain_fft_bins();
            if !bins.is_empty() {
                self.spectrum.feed_mags(&bins);
            } else {
                let pcm = self.player.drain_pcm();
                if !pcm.is_empty() {
                    self.spectrum.feed(&pcm);
                }
            }
            let vol = self.player.volume() as f32 / 100.0;
            let bpm = self
                .player
                .now_playing()
                .and_then(|t| t.bpm)
                .unwrap_or(120.0);
            let pos = self.elapsed_secs();
            let _ = self.spectrum.tick(dt, playing, vol, bpm, pos);
        }

        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.cover.is_some() && t.art.is_none())
        {
            self.hydrate_toast_art().await;
        }
        if self.toast_alpha().is_none() {
            self.toast = None;
        }

        if self.last_media.elapsed() > Duration::from_millis(1000) {
            self.publish_media();
            self.last_media = Instant::now();
        }
    }

    /// True while something is animating (toasts, popups, login, now-playing morph).
    /// Playback itself redraws at [`Self::playback_redraw_every`] — not 120 Hz.
    pub fn needs_frames(&self) -> bool {
        if self.toast.is_some() || self.popup.is_some() || self.loading || self.login.is_some() {
            return true;
        }
        if self.input_mode {
            return true;
        }
        let target = if self.now_playing_mode { 1.0 } else { 0.0 };
        (self.np_progress - target).abs() > 0.002
    }

    /// Target redraw interval while a track is playing.
    pub fn playback_redraw_every(&self) -> Duration {
        if self.player.status() != PlayerStatus::Playing {
            return Duration::from_millis(250);
        }
        if self.settings.show_spectrum {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(80)
        }
    }

    async fn handle_media(&mut self, cmd: MediaCommand) {
        match cmd {
            MediaCommand::Play | MediaCommand::PlayPause => self.toggle_pause(),
            MediaCommand::Pause => {
                if self.player.status() == PlayerStatus::Playing {
                    self.toggle_pause();
                }
            }
            MediaCommand::Stop => self.stop(),
            MediaCommand::Next => self.play_next().await,
            MediaCommand::Previous => self.play_previous().await,
            MediaCommand::SeekBy { seconds } => {
                let _ = self.player.seek_by(seconds);
            }
            MediaCommand::SeekTo { seconds } => {
                let _ = self.player.seek(seconds);
            }
        }
    }

    fn publish_media(&mut self) {
        let track = self.player.now_playing().cloned();
        // Linux MPRIS can fetch HTTP covers; macOS Now Playing cannot (ATS).
        #[cfg(target_os = "linux")]
        let cover_url = self
            .media_cover_url
            .clone()
            .or_else(|| track.as_ref().and_then(|t| t.cover_url(320)));
        #[cfg(not(target_os = "linux"))]
        let cover_url = self.media_cover_url.clone();
        let position = Duration::from_secs_f64(self.elapsed_secs());
        let np = MediaNowPlaying {
            status: self.player.status(),
            volume: self.player.volume(),
            position,
            cover_url,
            shuffle: self.player.shuffle() != ShuffleMode::Off,
            loop_all: self.player.repeat() == RepeatMode::All,
            loop_one: self.player.repeat() == RepeatMode::One,
            track,
        };
        self.media.publish(&np);
    }

    async fn finish_login(&mut self) {
        self.login = None;
        match TidalService::restore(self.config.clone(), self.settings.quality).await {
            Ok(Some(service)) => {
                let who = service.username().unwrap_or_else(|| "your account".into());
                self.service = Some(service);
                self.screen = Screen::Browse;
                self.set_status(format!("Signed in as {who}"));
                self.load_library().await;
                self.tab = if self.favorites.is_empty() {
                    Tab::Search
                } else {
                    Tab::Favorites
                };
                self.select_first();
            }
            Ok(None) | Err(_) => {
                self.set_status("Login completed but no session was saved — try again".into());
                self.start_login();
            }
        }
    }

    pub async fn load_library(&mut self) {
        self.load_favorites().await;
        self.load_playlists().await;
        self.load_home().await;
        self.recompute_filter();
    }

    pub async fn load_favorites(&mut self) {
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        let tracks = self
            .service
            .as_ref()
            .unwrap()
            .favorite_tracks_all(10_000)
            .await;
        let albums = self
            .service
            .as_ref()
            .unwrap()
            .favorite_albums_all(10_000)
            .await;
        let artists = self
            .service
            .as_ref()
            .unwrap()
            .favorite_artists_all(10_000)
            .await;
        self.loading = false;
        let mut parts = Vec::new();
        match tracks {
            Ok(tracks) => {
                self.loved = tracks.iter().map(|t| t.id).collect();
                let n = tracks.len();
                self.favorites = tracks;
                parts.push(format!("{n} song(s)"));
            }
            Err(e) => parts.push(format!("songs: {e}")),
        }
        match albums {
            Ok(albums) => {
                let n = albums.len();
                self.fav_albums = albums;
                parts.push(format!("{n} album(s)"));
            }
            Err(e) => parts.push(format!("albums: {e}")),
        }
        match artists {
            Ok(artists) => {
                let n = artists.len();
                self.fav_artists = artists;
                parts.push(format!("{n} artist(s)"));
            }
            Err(e) => parts.push(format!("artists: {e}")),
        }
        self.set_status(format!("Library: {}", parts.join(" · ")));
    }

    pub async fn load_playlists(&mut self) {
        if self.service.is_none() {
            return;
        }
        match self.service.as_ref().unwrap().playlists().await {
            Ok(p) => self.playlists = p,
            Err(e) => self.set_status(format!("Playlists: {e}")),
        }
    }

    pub async fn load_home(&mut self) {
        if self.service.is_none() {
            return;
        }
        match self.service.as_ref().unwrap().home_cards().await {
            Ok((mixes, cards)) => {
                self.mixes = mixes;
                self.for_you = cards;
                self.home_error = None;
                if self.mixes.is_empty() && self.for_you.is_empty() {
                    self.home_error = Some("TIDAL returned no mixes or For You cards.".into());
                }
            }
            Err(e) => {
                self.home_error = Some(e.to_string());
                self.set_status(format!("Home feed: {e}"));
            }
        }
    }

    pub async fn submit_search(&mut self) {
        let query = self.input.trim().to_string();
        self.input_mode = false;
        if query.is_empty() || self.service.is_none() {
            return;
        }
        self.loading = true;
        self.set_status(format!("Searching for “{query}”…"));
        let result = self.service.as_ref().unwrap().search(&query, 25).await;
        self.loading = false;
        match result {
            Ok(results) => {
                self.set_status(format!("{} hit(s) for “{query}”", results.total()));
                self.results = results;
                self.tab = Tab::Search;
                self.nav.clear();
                // Catalog already narrowed the list; show every hit.
                self.input.clear();
                self.set_filter_hits(None);
                self.select_first();
            }
            Err(e) => self.set_status(format!("Search failed: {e}")),
        }
    }

    pub async fn activate(&mut self) {
        if !self.nav.is_empty() {
            self.play_selected().await;
            return;
        }
        match self.tab {
            Tab::Search => match self.search_scope {
                SearchScope::Tracks => self.play_selected().await,
                SearchScope::Albums => {
                    if let Some(a) = self.results.albums.get(self.sel_orig()).cloned() {
                        self.open_album(a.id, a.title).await;
                    }
                }
                SearchScope::Artists => {
                    if let Some(a) = self.results.artists.get(self.sel_orig()).cloned() {
                        self.open_artist(a.id, a.name).await;
                    }
                }
                SearchScope::Playlists => {
                    if let Some(p) = self.results.playlists.get(self.sel_orig()).cloned() {
                        self.open_playlist(p.uuid, p.title).await;
                    }
                }
            },
            Tab::Home => {
                if let Some(c) = self.for_you.get(self.sel_orig()).cloned() {
                    self.open_card(c).await;
                }
            }
            Tab::Mixes => {
                if let Some(m) = self.mixes.get(self.sel_orig()).cloned() {
                    self.open_mix(m.id, m.title).await;
                }
            }
            Tab::Library => self.play_selected().await,
            Tab::Playlists => {
                if let Some(p) = self.playlists.get(self.sel_orig()).cloned() {
                    self.open_playlist(p.uuid, p.title).await;
                }
            }
            Tab::Favorites => match self.fav_section {
                FavSection::Tracks => self.play_selected().await,
                FavSection::Albums => {
                    if let Some(a) = self.fav_albums.get(self.sel_orig()).cloned() {
                        self.open_album(a.id, a.title).await;
                    }
                }
                FavSection::Artists => {
                    if let Some(a) = self.fav_artists.get(self.sel_orig()).cloned() {
                        self.open_artist(a.id, a.name).await;
                    }
                }
            },
            Tab::Queue => self.play_selected().await,
        }
    }

    async fn open_card(&mut self, card: HomeCard) {
        match card.kind {
            HomeCardKind::Mix { id } => self.open_mix(id, card.title).await,
            HomeCardKind::Playlist { uuid } => self.open_playlist(uuid, card.title).await,
            HomeCardKind::Album { id } => self.open_album(id, card.title).await,
            HomeCardKind::Artist { id } => self.open_artist(id, card.title).await,
        }
    }

    pub async fn open_playlist(&mut self, uuid: String, title: String) {
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        let result = self.service.as_ref().unwrap().playlist_tracks(&uuid).await;
        self.loading = false;
        match result {
            Ok(tracks) => {
                self.page_tracks = tracks;
                self.nav.push(Page::Playlist { uuid, title });
                self.clear_filter();
                self.select_first();
            }
            Err(e) => self.set_status(format!("Playlist: {e}")),
        }
    }

    async fn open_mix(&mut self, id: String, title: String) {
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        let result = self.service.as_ref().unwrap().mix_tracks(&id).await;
        self.loading = false;
        match result {
            Ok(tracks) => {
                self.page_tracks = tracks;
                self.nav.push(Page::Mix { id, title });
                self.clear_filter();
                self.select_first();
            }
            Err(e) => self.set_status(format!("Mix: {e}")),
        }
    }

    async fn open_album(&mut self, id: u64, title: String) {
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        let result = self.service.as_ref().unwrap().album_tracks(id).await;
        self.loading = false;
        match result {
            Ok(tracks) => {
                self.page_tracks = tracks;
                self.nav.push(Page::Album { id, title });
                self.clear_filter();
                self.select_first();
            }
            Err(e) => self.set_status(format!("Album: {e}")),
        }
    }

    async fn open_artist(&mut self, id: u64, name: String) {
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        let profile = self.service.as_ref().unwrap().artist(id).await;
        let tracks = self.service.as_ref().unwrap().artist_tracks(id).await;
        let albums = self.service.as_ref().unwrap().artist_albums(id).await;
        let bio = self.service.as_ref().unwrap().artist_bio(id).await;
        self.loading = false;
        let mut errors = Vec::new();
        self.page_tracks = match tracks {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("songs: {e}"));
                Vec::new()
            }
        };
        self.page_albums = match albums {
            Ok(a) => a,
            Err(e) => {
                errors.push(format!("albums: {e}"));
                Vec::new()
            }
        };
        self.page_bio = bio.unwrap_or_default();
        self.page_artist = match profile {
            Ok(a) => Some(a),
            Err(e) => {
                errors.push(format!("profile: {e}"));
                Some(ArtistView {
                    id,
                    name: name.clone(),
                    picture: None,
                    mix_id: None,
                })
            }
        };
        self.page_art = None;
        if let Some(pic) = self
            .page_artist
            .as_ref()
            .and_then(|a| a.picture.clone())
            .or_else(|| self.page_albums.first().and_then(|a| a.cover.clone()))
        {
            self.page_art = self.art.protocol_for(&pic).await;
        }
        self.nav.push(Page::Artist { id, name });
        self.clear_filter();
        self.select_first();
        if !errors.is_empty() {
            self.set_status(format!("Artist: {}", errors.join("; ")));
        } else if self.page_tracks.is_empty() && self.page_albums.is_empty() {
            self.set_status("Artist loaded with no songs or albums yet".into());
        }
    }

    pub async fn open_page_album(&mut self, index: usize) {
        if let Some(a) = self.page_albums.get(index).cloned() {
            self.open_album(a.id, a.title).await;
        }
    }

    pub fn go_back(&mut self) -> bool {
        if self.popup.is_some() {
            self.close_popup();
            return true;
        }
        if self.now_playing_mode {
            self.now_playing_mode = false;
            return true;
        }
        if self.input_mode {
            self.input_mode = false;
            return true;
        }
        if !self.nav.is_empty() {
            self.nav.pop();
            self.page_tracks.clear();
            self.page_albums.clear();
            self.page_bio.clear();
            self.page_artist = None;
            self.page_art = None;
            self.recompute_filter();
            self.select_first();
            return true;
        }
        false
    }

    pub async fn play_selected(&mut self) {
        let tracks = self.unfiltered_tracks();
        if tracks.is_empty() {
            return;
        }
        let orig = self.sel_orig().min(tracks.len().saturating_sub(1));
        if self.tab == Tab::Queue && self.nav.is_empty() {
            let _ = self.player.select(orig);
        } else {
            self.player.set_queue(tracks, orig);
            self.player
                .set_shuffle(self.settings.shuffle, Some(&self.counts));
        }
        self.persist_queue();
        self.play_current().await;
    }

    pub async fn add_selected(&mut self) {
        let (tracks, idx) = self.current_tracks_with_selection();
        if let Some(t) = tracks.get(idx).cloned() {
            self.player.enqueue(t.clone());
            self.persist_queue();
            self.set_status(format!("Queued {}", t.label()));
        }
    }

    pub async fn add_all(&mut self) {
        let (tracks, _) = self.current_tracks_with_selection();
        let n = tracks.len();
        if n == 0 {
            return;
        }
        self.player.enqueue_all(tracks);
        self.persist_queue();
        self.set_status(format!("Queued {n} tracks"));
    }

    async fn play_current(&mut self) {
        let Some(track) = self.player.current().cloned() else {
            return;
        };
        let quality = self.settings.quality;
        let Some(service) = self.service.as_mut() else {
            return;
        };
        let stream = service.stream_url_for(&track, quality).await;
        match stream {
            Ok(info) => match self
                .player
                .play_current_with_quality(&info.url, info.quality.clone())
            {
                Ok(()) => {
                    self.stream_quality = info.quality;
                    self.spectrum.set_seed(track.id);
                    self.notify_track(&track);
                    self.load_now_art(track.cover.clone()).await;
                    self.load_lyrics(track.id).await;
                    self.publish_media();
                    self.persist_queue();
                }
                Err(e) => self.set_status(format!("Playback error: {e}")),
            },
            Err(e) => self.set_status(format!("Could not get stream: {e}")),
        }
    }

    async fn load_lyrics(&mut self, id: u64) {
        self.lyrics.clear();
        if let Some(service) = self.service.as_ref() {
            if let Ok(lines) = service.lyrics(id).await {
                self.lyrics = lines;
            }
        }
    }

    async fn load_now_art(&mut self, cover: Option<String>) {
        self.now_art = match cover.as_ref() {
            Some(id) => self.art.protocol_for(id).await,
            None => None,
        };
        if let Some(old) = self.media_cover_path.take() {
            let _ = std::fs::remove_file(old);
        }
        self.media_cover_url = None;
        if let Some(id) = cover.as_ref() {
            let url = tiders_core::images::cover_url(id, 320);
            if let Ok(bytes) = tiders_core::images::fetch(&url).await {
                if let Some(path) = write_exclusive_cover(id, &bytes) {
                    self.media_cover_url = tiders_core::media::file_url(&path);
                    self.media_cover_path = Some(path);
                }
            }
        }
        self.publish_media();
    }

    pub async fn play_next(&mut self) {
        if self.player.next_track().is_some() {
            self.play_current().await;
        }
    }

    pub async fn play_previous(&mut self) {
        if self.elapsed_secs() > 3.0 {
            let _ = self.player.seek(0.0);
            return;
        }
        if self.player.previous_track().is_some() {
            self.play_current().await;
        }
    }

    pub fn toggle_pause(&mut self) {
        match self.player.status() {
            PlayerStatus::Playing => {
                let _ = self.player.pause();
                self.set_status("Paused".into());
            }
            PlayerStatus::Paused => {
                let _ = self.player.resume();
                self.set_status("Resumed".into());
            }
            PlayerStatus::Stopped => {}
        }
        self.publish_media();
    }

    pub fn stop(&mut self) {
        let _ = self.player.stop();
        self.now_art = None;
        self.lyrics.clear();
        self.set_status("Stopped".into());
        self.publish_media();
    }

    pub fn volume_up(&mut self) {
        let _ = self.player.volume_up(5);
        self.set_status(format!("Volume {}%", self.player.volume()));
    }

    pub fn volume_down(&mut self) {
        let _ = self.player.volume_down(5);
        self.set_status(format!("Volume {}%", self.player.volume()));
    }

    pub fn seek_by(&mut self, delta: f64) {
        let _ = self.player.seek_by(delta);
    }

    pub fn cycle_shuffle(&mut self) {
        let mode = self.player.cycle_shuffle(Some(&self.counts));
        self.settings.shuffle = mode;
        let _ = self.config.save_settings(&self.settings);
        self.set_status(format!("Shuffle {}", mode.label()));
        self.persist_queue();
    }

    pub fn cycle_repeat(&mut self) {
        let mode = self.player.cycle_repeat();
        self.settings.repeat = mode;
        let _ = self.config.save_settings(&self.settings);
        self.set_status(format!("Repeat {}", mode.label()));
    }

    pub async fn start_radio(&mut self) {
        let (tracks, idx) = self.current_tracks_with_selection();
        let Some(seed) = tracks.get(idx).cloned() else {
            return;
        };
        if self.service.is_none() {
            return;
        }
        self.loading = true;
        let radio = self.service.as_ref().unwrap().track_radio(seed.id).await;
        self.loading = false;
        match radio {
            Ok(mut items) if !items.is_empty() => {
                items.insert(0, seed.clone());
                self.player.set_queue(items, 0);
                self.persist_queue();
                self.set_status(format!("Radio from {}", seed.title));
                self.play_current().await;
            }
            Ok(_) => self.set_status("Radio returned no tracks".into()),
            Err(e) => self.set_status(format!("Radio: {e}")),
        }
    }

    pub async fn toggle_love(&mut self) {
        let Some(track) = self
            .selected_track()
            .or_else(|| self.player.now_playing().cloned())
        else {
            return;
        };
        self.apply_love(&track, false).await;
    }

    pub fn toggle_now_playing_mode(&mut self) {
        self.now_playing_mode = !self.now_playing_mode;
        if self.now_playing_mode {
            self.focus = Focus::NpQueue;
            self.sync_np_queue_selection();
        } else if self.focus == Focus::NpQueue {
            self.focus = Focus::Main;
        }
    }

    pub fn sync_np_queue_selection(&mut self) {
        let cur = self.player.queue().cursor();
        self.np_queue_state.select(cur);
    }

    pub fn move_np_queue(&mut self, delta: isize) {
        let len = self.player.queue().len();
        if len == 0 {
            self.np_queue_state.select(None);
            return;
        }
        let current = self.np_queue_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.np_queue_state.select(Some(next));
    }

    pub async fn activate_np_queue(&mut self) {
        let Some(i) = self.np_queue_state.selected() else {
            return;
        };
        if self.player.select(i).is_some() {
            self.persist_queue();
            self.play_current().await;
        }
    }

    pub fn toggle_sidebar(&mut self) {
        self.settings.sidebar_visible = !self.settings.sidebar_visible;
        let _ = self.config.save_settings(&self.settings);
        if !self.settings.sidebar_visible && self.focus == Focus::Sidebar {
            self.focus = Focus::Main;
        }
    }

    pub fn cycle_sort(&mut self) {
        if self.showing_tracks() {
            self.sort_key = self.sort_key.cycle();
            self.set_status(format!(
                "Sort by {} {}",
                self.sort_key.title(),
                if self.sort_asc { "↑" } else { "↓" }
            ));
        }
    }

    pub fn set_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_key = key;
            self.sort_asc = true;
        }
        self.set_status(format!(
            "Sort by {} {}",
            self.sort_key.title(),
            if self.sort_asc { "↑" } else { "↓" }
        ));
    }

    pub fn open_context(&mut self, x: u16, y: u16, track: TrackView) {
        self.open_popup(Popup::Context(Box::new(ContextMenu {
            x,
            y,
            cursor: 0,
            track,
        })));
    }

    /// Keyboard entry for the track context menu (`c`, Shift+F10, Menu).
    pub fn open_context_for_selection(&mut self) {
        let Some(track) = self
            .selected_track()
            .or_else(|| self.player.now_playing().cloned())
        else {
            self.set_status("No track for the context menu".into());
            return;
        };
        let (x, y) = self.context_anchor();
        self.open_context(x, y, track);
    }

    fn context_anchor(&self) -> (u16, u16) {
        if let Some(list) = self.hits.list {
            let sel = self.list_state.selected().unwrap_or(0);
            let row = sel.saturating_sub(self.hits.list_offset) as u16;
            return (
                list.x.saturating_add(2),
                list.y.saturating_add(row).saturating_sub(1),
            );
        }
        if let Some(queue) = self.hits.queue {
            let sel = self.np_queue_state.selected().unwrap_or(0);
            let row = sel.saturating_sub(self.hits.queue_offset) as u16;
            return (
                queue.x.saturating_add(2),
                queue.y.saturating_add(row).saturating_sub(1),
            );
        }
        (8, 6)
    }

    pub fn context_move(&mut self, delta: isize) {
        if let Some(Popup::Context(menu)) = self.popup.as_mut() {
            let n = ContextAction::all().len() as isize;
            menu.cursor = (menu.cursor as isize + delta).rem_euclid(n) as usize;
        }
    }

    pub async fn context_activate(&mut self) {
        let Some(Popup::Context(menu)) = self.popup.take() else {
            return;
        };
        self.popup_opened = None;
        let action = ContextAction::all()[menu.cursor.min(ContextAction::all().len() - 1)];
        self.run_context_action(action, menu.track).await;
    }

    pub async fn run_context_action(&mut self, action: ContextAction, track: TrackView) {
        match action {
            ContextAction::GoArtist => {
                if let Some(id) = track.artist_id {
                    self.open_artist(id, track.artist).await;
                } else {
                    self.set_status("No artist id on this track".into());
                }
            }
            ContextAction::GoAlbum => {
                if let (Some(id), Some(title)) = (track.album_id, track.album.clone()) {
                    self.open_album(id, title).await;
                } else {
                    self.set_status("No album id on this track".into());
                }
            }
            ContextAction::ToggleFav | ContextAction::Like => {
                self.apply_love(&track, action == ContextAction::Like).await;
            }
            ContextAction::Dislike => {
                if let Some(service) = self.service.as_ref() {
                    let _ = service.dislike_track(track.id).await;
                }
                if self.loved.contains(&track.id) {
                    if let Some(service) = self.service.as_ref() {
                        let _ = service.unlove_track(track.id).await;
                    }
                    self.loved.remove(&track.id);
                }
                self.set_status(format!("Don't like {}", track.title));
                if self.player.now_playing().is_some_and(|t| t.id == track.id) {
                    self.play_next().await;
                }
            }
        }
    }

    async fn apply_love(&mut self, track: &TrackView, force_like: bool) {
        if self.service.is_none() {
            return;
        }
        let loved = self.loved.contains(&track.id);
        if force_like && loved {
            self.set_status(format!("Already in favorites: {}", track.title));
            return;
        }
        let result = if loved {
            self.service.as_ref().unwrap().unlove_track(track.id).await
        } else {
            self.service.as_ref().unwrap().love_track(track.id).await
        };
        match result {
            Ok(()) => {
                if loved {
                    self.loved.remove(&track.id);
                    self.set_status(format!("Removed {} from favorites", track.title));
                } else {
                    self.loved.insert(track.id);
                    self.set_status(format!("Added {} to favorites", track.title));
                }
            }
            Err(e) => self.set_status(format!("Favorite: {e}")),
        }
    }

    pub fn toggle_spectrum(&mut self) {
        self.settings.show_spectrum = !self.settings.show_spectrum;
        let _ = self.config.save_settings(&self.settings);
    }

    pub fn cycle_eq_theme(&mut self) {
        self.settings.eq_theme = self.settings.eq_theme.cycle();
        let _ = self.config.save_settings(&self.settings);
        self.set_status(format!("EQ theme {}", self.settings.eq_theme.label()));
    }

    pub fn elapsed_secs(&mut self) -> f64 {
        self.player.position().unwrap_or(0.0)
    }

    pub fn progress(&mut self) -> f64 {
        let dur = self.player.duration().unwrap_or(0.0);
        if dur <= 0.0 {
            0.0
        } else {
            (self.elapsed_secs() / dur).clamp(0.0, 1.0)
        }
    }

    pub fn popup_open(&self) -> bool {
        self.popup.is_some()
    }

    pub fn popup_factor(&self) -> f32 {
        let Some(t0) = self.popup_opened else {
            return 1.0;
        };
        super::anim::ease_out_quint(t0.elapsed().as_secs_f32() / POPUP_OPEN_SECS)
    }

    fn open_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
        self.popup_opened = Some(Instant::now());
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

    pub async fn open_detail(&mut self) {
        let Some(track) = self.selected_track() else {
            return;
        };
        self.detail_art = match track.cover.clone() {
            Some(id) => self.art.protocol_for(&id).await,
            None => None,
        };
        self.open_popup(Popup::Detail(Box::new(track)));
    }

    pub fn close_popup(&mut self) {
        self.popup = None;
        self.popup_opened = None;
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
            self.set_status(format!("Quality set to {}", q.short_label()));
        }
        self.close_popup();
    }

    pub fn set_status(&mut self, msg: String) {
        self.toast = Some(Toast {
            title: msg.clone(),
            subtitle: None,
            cover: None,
            art: None,
            started: Instant::now(),
        });
        self.status = msg;
    }

    /// Now-playing toast: title + artist, with cover art.
    pub fn notify_track(&mut self, track: &TrackView) {
        self.toast = Some(Toast {
            title: track.title.clone(),
            subtitle: Some(track.artist.clone()),
            cover: track.cover.clone(),
            art: None,
            started: Instant::now(),
        });
        self.status = format!("▶ {}", track.label());
    }

    pub fn toast_alpha(&self) -> Option<f32> {
        let toast = self.toast.as_ref()?;
        let age = toast.started.elapsed().as_secs_f32();
        if age >= TOAST_SECS {
            return None;
        }
        if age < TOAST_SECS * 0.55 {
            Some(1.0)
        } else {
            Some((1.0 - (age - TOAST_SECS * 0.55) / (TOAST_SECS * 0.45)).clamp(0.0, 1.0))
        }
    }

    pub async fn hydrate_toast_art(&mut self) {
        let Some(id) = self
            .toast
            .as_ref()
            .and_then(|t| t.cover.clone())
            .filter(|_| self.toast.as_ref().is_some_and(|t| t.art.is_none()))
        else {
            return;
        };
        if let Some(art) = self.art.protocol_for(&id).await {
            if let Some(toast) = self.toast.as_mut() {
                toast.art = Some(art);
            }
        } else if let Some(toast) = self.toast.as_mut() {
            toast.cover = None;
        }
    }

    fn sel(&self) -> usize {
        self.list_state.selected().unwrap_or(0)
    }

    /// Map a filtered/sorted row index back onto the unfiltered source list.
    pub fn orig_at(&self, filtered: usize) -> usize {
        self.view_indices()
            .get(filtered)
            .copied()
            .unwrap_or(filtered)
    }

    pub fn view_indices(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = match &self.filter_hits {
            Some(hits) => hits.iter().map(|h| h.index).collect(),
            None => (0..self.unfiltered_len()).collect(),
        };
        if self.showing_tracks() {
            let tracks = self.unfiltered_tracks();
            idx.sort_by(|&a, &b| {
                let cmp = cmp_tracks(tracks.get(a), tracks.get(b), self.sort_key);
                if self.sort_asc {
                    cmp
                } else {
                    cmp.reverse()
                }
            });
        }
        idx
    }

    /// Map the highlighted (filtered) row back onto the unfiltered source list.
    fn sel_orig(&self) -> usize {
        self.orig_at(self.sel())
    }

    pub fn filter_active(&self) -> bool {
        self.filter_hits.is_some()
    }

    pub fn hit_for_source(&self, src: usize) -> Option<&super::filter::Hit> {
        let hits = self.filter_hits.as_ref()?;
        hits.get(*self.filter_by_src.get(&src)?)
    }

    pub fn clear_filter(&mut self) {
        self.input.clear();
        self.set_filter_hits(None);
    }

    /// Re-rank the visible corpus against `self.input` (FFF / neo_frizbee).
    pub fn recompute_filter(&mut self) {
        let needle = self.input.trim();
        if needle.is_empty() {
            self.set_filter_hits(None);
            return;
        }
        let hay = self.collect_haystacks();
        self.set_filter_hits(Some(super::filter::rank(needle, &hay)));
    }

    fn set_filter_hits(&mut self, hits: Option<Vec<super::filter::Hit>>) {
        self.filter_by_src.clear();
        if let Some(ref hits) = hits {
            self.filter_by_src
                .extend(hits.iter().enumerate().map(|(i, h)| (h.index, i)));
        }
        self.filter_hits = hits;
    }

    fn collect_haystacks(&self) -> Vec<String> {
        if !self.nav.is_empty() {
            return self.page_tracks.iter().map(track_haystack).collect();
        }
        match self.tab {
            Tab::Search => match self.search_scope {
                SearchScope::Tracks => self.results.tracks.iter().map(track_haystack).collect(),
                SearchScope::Albums => self
                    .results
                    .albums
                    .iter()
                    .map(|a| format!("{} {}", a.title, a.artist))
                    .collect(),
                SearchScope::Artists => self
                    .results
                    .artists
                    .iter()
                    .map(|a| a.name.clone())
                    .collect(),
                SearchScope::Playlists => self
                    .results
                    .playlists
                    .iter()
                    .map(|p| format!("{} {}", p.title, p.description.clone().unwrap_or_default()))
                    .collect(),
            },
            Tab::Home => self
                .for_you
                .iter()
                .map(|c| format!("{} {}", c.title, c.subtitle))
                .collect(),
            Tab::Mixes => self
                .mixes
                .iter()
                .map(|m| format!("{} {}", m.title, m.subtitle))
                .collect(),
            Tab::Library => self.favorites.iter().map(track_haystack).collect(),
            Tab::Playlists => self
                .playlists
                .iter()
                .map(|p| format!("{} {}", p.title, p.description.clone().unwrap_or_default()))
                .collect(),
            Tab::Favorites => match self.fav_section {
                FavSection::Tracks => self.favorites.iter().map(track_haystack).collect(),
                FavSection::Albums => self
                    .fav_albums
                    .iter()
                    .map(|a| format!("{} {}", a.title, a.artist))
                    .collect(),
                FavSection::Artists => self.fav_artists.iter().map(|a| a.name.clone()).collect(),
            },
            Tab::Queue => self
                .player
                .queue()
                .items()
                .iter()
                .map(track_haystack)
                .collect(),
        }
    }

    pub fn signed_in(&self) -> bool {
        self.service.is_some()
    }

    pub fn artist_page(&self) -> bool {
        matches!(self.nav.last(), Some(Page::Artist { .. }))
    }

    pub fn selected_track(&self) -> Option<TrackView> {
        let (tracks, idx) = self.current_tracks_with_selection();
        tracks.get(idx).cloned()
    }

    pub fn current_tracks_with_selection(&self) -> (Vec<TrackView>, usize) {
        let tracks = self.current_tracks();
        let idx = self
            .list_state
            .selected()
            .unwrap_or(0)
            .min(tracks.len().saturating_sub(1));
        (tracks, idx)
    }

    pub fn showing_tracks(&self) -> bool {
        !self.nav.is_empty()
            || matches!(self.tab, Tab::Queue | Tab::Library)
            || (self.tab == Tab::Favorites && self.fav_section == FavSection::Tracks)
            || (self.tab == Tab::Search && self.search_scope == SearchScope::Tracks)
    }

    pub fn unfiltered_tracks(&self) -> Vec<TrackView> {
        if !self.nav.is_empty() {
            return self.page_tracks.clone();
        }
        match self.tab {
            Tab::Search if self.search_scope == SearchScope::Tracks => self.results.tracks.clone(),
            Tab::Library => self.favorites.clone(),
            Tab::Favorites if self.fav_section == FavSection::Tracks => self.favorites.clone(),
            Tab::Queue => self.player.queue().items().to_vec(),
            _ => Vec::new(),
        }
    }

    pub fn current_tracks(&self) -> Vec<TrackView> {
        let all = self.unfiltered_tracks();
        self.view_indices()
            .into_iter()
            .filter_map(|i| all.get(i).cloned())
            .collect()
    }

    pub fn unfiltered_len(&self) -> usize {
        if !self.nav.is_empty() {
            return self.page_tracks.len();
        }
        match self.tab {
            Tab::Search => match self.search_scope {
                SearchScope::Tracks => self.results.tracks.len(),
                SearchScope::Albums => self.results.albums.len(),
                SearchScope::Artists => self.results.artists.len(),
                SearchScope::Playlists => self.results.playlists.len(),
            },
            Tab::Home => self.for_you.len(),
            Tab::Mixes => self.mixes.len(),
            Tab::Library => self.favorites.len(),
            Tab::Playlists => self.playlists.len(),
            Tab::Favorites => match self.fav_section {
                FavSection::Tracks => self.favorites.len(),
                FavSection::Albums => self.fav_albums.len(),
                FavSection::Artists => self.fav_artists.len(),
            },
            Tab::Queue => self.player.queue().len(),
        }
    }

    pub fn current_len(&self) -> usize {
        self.filter_hits
            .as_ref()
            .map(|h| h.len())
            .unwrap_or_else(|| self.unfiltered_len())
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.nav.clear();
        self.page_artist = None;
        self.page_art = None;
        self.focus = Focus::Main;
        self.clear_filter();
        self.select_first();
        if tab == Tab::Queue {
            if let Some(cur) = self.player.queue().cursor() {
                self.list_state.select(Some(cur));
            }
        }
    }

    pub fn set_fav_section(&mut self, section: FavSection) {
        self.set_tab(Tab::Favorites);
        self.fav_section = section;
        self.clear_filter();
        self.select_first();
    }

    pub fn seek_ratio(&mut self, ratio: f64) {
        let Some(dur) = self.player.duration() else {
            return;
        };
        let _ = self.player.seek((dur * ratio.clamp(0.0, 1.0)).max(0.0));
    }

    pub fn select_index(&mut self, index: usize) {
        let len = self.current_len();
        if len == 0 {
            self.list_state.select(None);
            return;
        }
        self.list_state.select(Some(index.min(len - 1)));
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

    pub fn page_title(&self) -> String {
        if let Some(page) = self.nav.last() {
            return match page {
                Page::Playlist { title, .. } => format!("Playlist · {title}"),
                Page::Mix { title, .. } => format!("Mix · {title}"),
                Page::Album { title, .. } => format!("Album · {title}"),
                Page::Artist { name, .. } => format!("Artist · {name}"),
            };
        }
        match self.tab {
            Tab::Search => format!("Search · {}", self.search_scope.title()),
            Tab::Home => "For You".into(),
            Tab::Mixes => "My Mixes".into(),
            Tab::Library => "Library · Songs".into(),
            Tab::Playlists => "Playlists".into(),
            Tab::Favorites => format!("Favorites · {}", self.fav_section.title()),
            Tab::Queue => "Queue".into(),
        }
    }

    fn persist_queue(&self) {
        let path = self.config.queue_path();
        let Ok(raw) = serde_json::to_string(self.player.queue()) else {
            return;
        };
        let _ = self.config.ensure_dir();
        // Keep the 120 Hz loop off the filesystem: serialize on the UI thread
        // (tiny JSON) and write from a blocking worker.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(move || {
                let _ = std::fs::write(path, raw);
            });
        } else {
            let _ = std::fs::write(path, raw);
        }
    }

    fn restore_queue(&mut self) {
        let path = self.config.queue_path();
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(q) = serde_json::from_str::<tiders_core::Queue>(&raw) {
                *self.player.queue_mut() = q;
            }
        }
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
        self.persist_queue();
        self.should_quit = true;
    }
}

fn track_haystack(track: &TrackView) -> String {
    match track.album.as_deref() {
        Some(album) if !album.is_empty() => format!("{} {} {}", track.title, track.artist, album),
        _ => format!("{} {}", track.title, track.artist),
    }
}

fn cmp_tracks(a: Option<&TrackView>, b: Option<&TrackView>, key: SortKey) -> std::cmp::Ordering {
    let Some(a) = a else {
        return std::cmp::Ordering::Greater;
    };
    let Some(b) = b else {
        return std::cmp::Ordering::Less;
    };
    match key {
        SortKey::Title => a
            .title
            .to_ascii_lowercase()
            .cmp(&b.title.to_ascii_lowercase()),
        SortKey::Artist => a
            .artist
            .to_ascii_lowercase()
            .cmp(&b.artist.to_ascii_lowercase())
            .then_with(|| {
                a.title
                    .to_ascii_lowercase()
                    .cmp(&b.title.to_ascii_lowercase())
            }),
        SortKey::Album => a
            .album
            .clone()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(&b.album.clone().unwrap_or_default().to_ascii_lowercase())
            .then_with(|| {
                a.title
                    .to_ascii_lowercase()
                    .cmp(&b.title.to_ascii_lowercase())
            }),
        SortKey::Time => a.duration_secs.cmp(&b.duration_secs).then_with(|| {
            a.title
                .to_ascii_lowercase()
                .cmp(&b.title.to_ascii_lowercase())
        }),
    }
}

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

/// Write cover bytes to a new file in the temp dir. `create_new` refuses to
/// follow/overwrite an existing path (including a symlink planted in `/tmp`).
fn write_exclusive_cover(cover_id: &str, bytes: &[u8]) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "tiders-cover-{}-{cover_id}-{stamp}.jpg",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()?;
    if file.write_all(bytes).is_err() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::super::filter::Hit;
    use std::collections::HashMap;

    fn index_hits(hits: &[Hit]) -> HashMap<usize, usize> {
        hits.iter().enumerate().map(|(i, h)| (h.index, i)).collect()
    }

    #[test]
    fn filter_by_src_survives_sorted_view_order() {
        let hits = vec![
            Hit {
                index: 4,
                indices: vec![0],
            },
            Hit {
                index: 1,
                indices: vec![2],
            },
            Hit {
                index: 7,
                indices: vec![1],
            },
        ];
        let by_src = index_hits(&hits);
        // Ranked order is 4, 1, 7; sorted visible order is 1, 4, 7.
        let visible = [1usize, 4, 7];
        let looked: Vec<&[usize]> = visible
            .iter()
            .map(|src| hits[*by_src.get(src).unwrap()].indices.as_slice())
            .collect();
        assert_eq!(looked, vec![&[2][..], &[0][..], &[1][..]]);
    }
}
