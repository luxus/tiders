//! Front-end-agnostic playback **engine**: one command bus, many transports.
//!
//! The TUI talks to [`Player`] in-process today. GUI shells (a Noctalia
//! plugin), a headless `tiders daemon`, and later **Sendspin** / **Music
//! Assistant** should all drive the *same* session, queue, and audio backend
//! without duplicating logic.
//!
//! ```text
//!   TUI  CLI  Noctalia          Sendspin roles (later)
//!     \   |   /                 controller / metadata /
//!      EngineCommand            artwork / visualizer / player
//!           |
//!      ┌────┴────┐
//!      │ Engine  │  TidalService + Player + MPRIS + downloads
//!      └────┬────┘
//!           │
//!      AudioBackend (mpv now; a Sendspin player sink later)
//! ```
//!
//! **Transports** (this crate):
//! - Unix JSON IPC ([`ipc`]) — Noctalia / `tiders ctl` / any local GUI
//! - MPRIS / Now Playing ([`crate::media`]) — already in-process
//!
//! **Later, without rewriting the engine:**
//! - A Sendspin *client* transport maps `controller@v1` messages onto
//!   [`EngineCommand`] and `metadata@v1` / `artwork@v1` / `visualizer@v1`
//!   onto [`EngineEvent`]. Music Assistant speaks Sendspin natively, so that
//!   one adapter covers "Tiders as an MA player / wall display".
//! - A Sendspin *player* [`crate::player::AudioBackend`] would receive
//!   timestamped PCM instead of a URL when Tiders is the speaker.
//! - A Music Assistant HTTP/WS client transport would forward the same
//!   commands when Tiders is only a remote control for an MA server.
//!
//! Hello frames advertise Sendspin-shaped `roles` so those adapters can be
//! added without changing the JSON schema Noctalia already uses.

pub mod ipc;

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::{Config, Quality, Settings};
use crate::download::{self, DownloadProgress, DownloadReport};
use crate::error::{Error, Result};
use crate::media::{MediaBridge, MediaCommand, MediaNowPlaying};
use crate::model::TrackView;
use crate::playcount::PlayCountStore;
use crate::player::{Player, PlayerState, PlayerStatus};
use crate::queue::{Queue, RepeatMode, ShuffleMode};
use crate::session::TidalService;
use crate::spectrum::{Spectrum, SpectrumFrame};

/// Wire protocol version. Bump when [`EngineCommand`] / [`EngineEvent`]
/// change in a breaking way; adapters should refuse unknown major versions.
pub const PROTOCOL_VERSION: u32 = 1;

/// Sendspin-shaped capability tags advertised on IPC hello.
///
/// These are **not** a Sendspin implementation — they document which engine
/// surfaces map onto which Sendspin roles so a future adapter is a thin
/// translator rather than a second player.
pub const ROLES: &[&str] = &["controller", "metadata", "artwork", "visualizer", "player"];

/// Commands every transport (IPC, MPRIS, future Sendspin controller) maps to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum EngineCommand {
    Status,
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek {
        seconds: f64,
    },
    SeekBy {
        seconds: f64,
    },
    SetVolume {
        volume: u8,
    },
    VolumeBy {
        delta: i16,
    },
    CycleShuffle,
    CycleRepeat,
    SetShuffle {
        mode: ShuffleMode,
    },
    SetRepeat {
        mode: RepeatMode,
    },
    PlayTrack {
        id: u64,
    },
    PlayPlaylist {
        uuid: String,
    },
    QueueAdd {
        id: u64,
    },
    QueueClear,
    SetQuality {
        quality: Quality,
    },
    DownloadPlaylist {
        uuid: String,
        #[serde(default)]
        dest: Option<PathBuf>,
    },
    Subscribe {
        #[serde(default)]
        spectrum: bool,
    },
    Quit,
}

/// Events pushed to subscribers (metadata / artwork / visualizer roles).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum EngineEvent {
    Hello {
        protocol: u32,
        app: String,
        roles: Vec<String>,
        transports: Vec<String>,
    },
    State {
        state: Box<EngineState>,
    },
    Spectrum {
        bars: Vec<f32>,
        peaks: Vec<f32>,
    },
    Download {
        index: usize,
        total: usize,
        title: String,
        status: String,
    },
    Error {
        message: String,
    },
}

/// Snapshot a GUI (or Sendspin metadata client) can render without extra
/// round-trips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineState {
    #[serde(flatten)]
    pub player: PlayerState,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub queue: Vec<TrackView>,
}

impl EngineEvent {
    pub fn hello() -> Self {
        EngineEvent::Hello {
            protocol: PROTOCOL_VERSION,
            app: "tiders".into(),
            roles: ROLES.iter().map(|s| (*s).to_string()).collect(),
            transports: vec!["ipc".into(), "mpris".into()],
        }
    }
}

/// Owns the TIDAL session, queue, audio backend, and OS media session.
pub struct Engine {
    pub config: Config,
    pub settings: Settings,
    service: TidalService,
    player: Player,
    media: MediaBridge,
    counts: PlayCountStore,
    spectrum: Spectrum,
    last_media: Instant,
    pub should_quit: bool,
}

impl Engine {
    pub async fn start(config: Config, settings: Settings) -> Result<Self> {
        let quality = settings.quality;
        let service = TidalService::restore(config.clone(), quality)
            .await?
            .ok_or(Error::NotAuthenticated)?;
        let mut player = Player::with_backend_replaygain(
            settings.backend,
            settings.volume,
            settings.replaygain.mpv_flag(),
        )?;
        if let Ok(raw) = std::fs::read_to_string(config.queue_path()) {
            if let Ok(q) = serde_json::from_str::<crate::queue::Queue>(&raw) {
                *player.queue_mut() = q;
            }
        }
        let counts = PlayCountStore::load(config.playcounts_path())?;
        Ok(Self {
            config,
            settings,
            service,
            player,
            media: MediaBridge::start(),
            counts,
            spectrum: Spectrum::new(48),
            last_media: Instant::now(),
            should_quit: false,
        })
    }

    pub fn player(&self) -> &Player {
        &self.player
    }

    pub fn player_mut(&mut self) -> &mut Player {
        &mut self.player
    }

    pub fn snapshot(&mut self) -> EngineState {
        let position_secs = self.player.position().unwrap_or(0.0);
        let duration_secs = self.player.duration().unwrap_or(0.0);
        EngineState {
            player: self.player.snapshot(),
            position_secs,
            duration_secs,
            queue: self.player.queue().items().to_vec(),
        }
    }

    pub fn state_event(&mut self) -> EngineEvent {
        EngineEvent::State {
            state: Box::new(self.snapshot()),
        }
    }

    pub async fn handle(&mut self, cmd: EngineCommand) -> Result<Option<DownloadReport>> {
        match cmd {
            EngineCommand::Status => Ok(None),
            EngineCommand::Play => {
                self.player.resume()?;
                Ok(None)
            }
            EngineCommand::Pause => {
                self.player.pause()?;
                Ok(None)
            }
            EngineCommand::PlayPause => {
                self.player.toggle_pause()?;
                Ok(None)
            }
            EngineCommand::Stop => {
                self.player.stop()?;
                Ok(None)
            }
            EngineCommand::Next => {
                if self.player.next_track().is_some() {
                    self.play_current().await?;
                }
                Ok(None)
            }
            EngineCommand::Previous => {
                if self.player.previous_track().is_some() {
                    self.play_current().await?;
                }
                Ok(None)
            }
            EngineCommand::Seek { seconds } => {
                self.player.seek(seconds)?;
                Ok(None)
            }
            EngineCommand::SeekBy { seconds } => {
                self.player.seek_by(seconds)?;
                Ok(None)
            }
            EngineCommand::SetVolume { volume } => {
                self.player.set_volume(volume)?;
                self.settings.volume = self.player.volume();
                let _ = self.config.save_settings(&self.settings);
                Ok(None)
            }
            EngineCommand::VolumeBy { delta } => {
                if delta >= 0 {
                    self.player.volume_up(delta as u8)?;
                } else {
                    self.player.volume_down(delta.unsigned_abs() as u8)?;
                }
                self.settings.volume = self.player.volume();
                let _ = self.config.save_settings(&self.settings);
                Ok(None)
            }
            EngineCommand::CycleShuffle => {
                let mode = self.player.cycle_shuffle(Some(&self.counts));
                self.settings.shuffle = mode;
                let _ = self.config.save_settings(&self.settings);
                Ok(None)
            }
            EngineCommand::CycleRepeat => {
                let mode = self.player.cycle_repeat();
                self.settings.repeat = mode;
                let _ = self.config.save_settings(&self.settings);
                Ok(None)
            }
            EngineCommand::SetShuffle { mode } => {
                self.player.set_shuffle(mode, Some(&self.counts));
                self.settings.shuffle = mode;
                let _ = self.config.save_settings(&self.settings);
                Ok(None)
            }
            EngineCommand::SetRepeat { mode } => {
                self.player.set_repeat(mode);
                self.settings.repeat = mode;
                let _ = self.config.save_settings(&self.settings);
                Ok(None)
            }
            EngineCommand::PlayTrack { id } => {
                let track = self.service.track(id).await?;
                self.player.set_queue(vec![track], 0);
                self.play_current().await?;
                Ok(None)
            }
            EngineCommand::PlayPlaylist { uuid } => {
                let tracks = self.service.playlist_tracks(&uuid).await?;
                if tracks.is_empty() {
                    return Err(Error::other("playlist is empty"));
                }
                self.player.set_queue(tracks, 0);
                self.play_current().await?;
                Ok(None)
            }
            EngineCommand::QueueAdd { id } => {
                let track = self.service.track(id).await?;
                self.player.enqueue(track);
                Ok(None)
            }
            EngineCommand::QueueClear => {
                self.player.queue_mut().replace(Vec::new(), 0);
                Ok(None)
            }
            EngineCommand::SetQuality { quality } => {
                self.settings.quality = quality;
                self.service.set_quality(quality);
                let _ = self.config.save_settings(&self.settings);
                Ok(None)
            }
            EngineCommand::DownloadPlaylist { uuid, dest } => {
                let dest = dest.unwrap_or_else(download::default_dest);
                let tracks = self.service.playlist_tracks(&uuid).await?;
                let report = download::download_tracks(
                    &mut self.service,
                    &tracks,
                    &dest,
                    self.settings.quality,
                    |_| {},
                )
                .await?;
                Ok(Some(report))
            }
            EngineCommand::Subscribe { .. } => Ok(None),
            EngineCommand::Quit => {
                let _ = self.player.stop();
                self.should_quit = true;
                Ok(None)
            }
        }
    }

    pub async fn play_current(&mut self) -> Result<()> {
        let Some(track) = self.player.current().cloned() else {
            return Ok(());
        };
        let info = self
            .service
            .stream_url_for(&track, self.settings.quality)
            .await?;
        self.player
            .play_current_with_quality(&info.url, info.quality.clone())?;
        self.spectrum.set_seed(track.id);
        save_queue(&self.config, self.player.queue());
        self.publish_media();
        Ok(())
    }

    pub async fn tick(&mut self, dt: f32) -> Option<SpectrumFrame> {
        for cmd in self.media.poll() {
            let mapped = match cmd {
                MediaCommand::Play => Some(EngineCommand::Play),
                MediaCommand::Pause => Some(EngineCommand::Pause),
                MediaCommand::PlayPause => Some(EngineCommand::PlayPause),
                MediaCommand::Stop => Some(EngineCommand::Stop),
                MediaCommand::Next => Some(EngineCommand::Next),
                MediaCommand::Previous => Some(EngineCommand::Previous),
                MediaCommand::SeekBy { seconds } => Some(EngineCommand::SeekBy { seconds }),
                MediaCommand::SeekTo { seconds } => Some(EngineCommand::Seek { seconds }),
            };
            if let Some(cmd) = mapped {
                let _ = self.handle(cmd).await;
            }
        }

        if self.player.poll_finished() && self.player.next_track().is_some() {
            let _ = self.play_current().await;
        }
        self.player.note_progress(dt as f64, Some(&mut self.counts));

        let playing = self.player.status() == PlayerStatus::Playing;
        let frame = if self.settings.show_spectrum {
            let pcm = self.player.drain_pcm();
            if !pcm.is_empty() {
                self.spectrum.feed(&pcm);
            }
            let vol = self.player.volume() as f32 / 100.0;
            let bpm = self
                .player
                .now_playing()
                .and_then(|t| t.bpm)
                .unwrap_or(120.0);
            let pos = self.player.position().unwrap_or(0.0);
            Some(self.spectrum.tick(dt, playing, vol, bpm, pos))
        } else {
            None
        };

        if self.last_media.elapsed() > std::time::Duration::from_millis(1000) {
            self.publish_media();
            self.last_media = Instant::now();
        }
        frame
    }

    fn publish_media(&mut self) {
        let position = std::time::Duration::from_secs_f64(self.player.position().unwrap_or(0.0));
        let np = MediaNowPlaying {
            track: self.player.now_playing().cloned(),
            status: self.player.status(),
            volume: self.player.volume(),
            position,
            cover_url: self.player.now_playing().and_then(|t| t.cover_url(320)),
            shuffle: self.player.shuffle() != ShuffleMode::Off,
            loop_all: self.player.repeat() == RepeatMode::All,
            loop_one: self.player.repeat() == RepeatMode::One,
        };
        self.media.publish(&np);
    }
}

/// Persist a queue JSON next to the session (best-effort).
pub fn save_queue(config: &Config, queue: &Queue) {
    let _ = config.ensure_dir();
    let path = config.queue_path();
    if let Ok(raw) = serde_json::to_string(queue) {
        let _ = std::fs::write(path, raw);
    }
}

pub fn progress_event(p: &DownloadProgress) -> EngineEvent {
    let status = match &p.status {
        download::DownloadStatus::Start => "start".into(),
        download::DownloadStatus::Done => "done".into(),
        download::DownloadStatus::Skip => "skip".into(),
        download::DownloadStatus::Failed(e) => format!("error:{e}"),
    };
    EngineEvent::Download {
        index: p.index,
        total: p.total,
        title: p.track.title.clone(),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrips_json() {
        let cmd = EngineCommand::PlayTrack { id: 42 };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"cmd\":\"play-track\""));
        let back: EngineCommand = serde_json::from_str(&json).unwrap();
        match back {
            EngineCommand::PlayTrack { id } => assert_eq!(id, 42),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hello_advertises_sendspin_shaped_roles() {
        let EngineEvent::Hello {
            roles, protocol, ..
        } = EngineEvent::hello()
        else {
            panic!("expected hello");
        };
        assert_eq!(protocol, PROTOCOL_VERSION);
        assert!(roles.contains(&"controller".into()));
        assert!(roles.contains(&"visualizer".into()));
        assert!(roles.contains(&"player".into()));
    }
}
