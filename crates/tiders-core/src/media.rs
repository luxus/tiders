//! OS media-control signaling (MPRIS on Linux, Now Playing on macOS).
//!
//! Desktop environments, `playerctl`, GNOME/KDE media applets, and macOS
//! Control Center / media keys talk to the player through this bridge. Failures
//! are swallowed: a missing D-Bus session (CI, SSH) must never block playback.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use crate::model::TrackView;
use crate::player::PlayerStatus;

/// A command coming *from* the OS (media keys, `playerctl`, Control Center).
#[derive(Debug, Clone)]
pub enum MediaCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative seek (seconds, may be negative).
    SeekBy {
        seconds: f64,
    },
    /// Absolute seek (seconds from start).
    SeekTo {
        seconds: f64,
    },
}

/// Snapshot we push *to* the OS so applets show the right track.
#[derive(Debug, Clone)]
pub struct MediaNowPlaying {
    pub track: Option<TrackView>,
    pub status: PlayerStatus,
    pub volume: u8,
    pub position: Duration,
    pub cover_url: Option<String>,
    pub shuffle: bool,
    pub loop_all: bool,
    pub loop_one: bool,
}

/// Best-effort handle to the platform media session.
pub struct MediaBridge {
    cmd_rx: Receiver<MediaCommand>,
    #[allow(dead_code)]
    inner: Option<Inner>,
}

struct Inner {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    controls: souvlaki::MediaControls,
}

impl MediaBridge {
    /// Attach to the session bus / Now Playing. Returns a dormant bridge when
    /// the platform rejects us (no D-Bus, no AppDelegate, …).
    pub fn start() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        match attach(cmd_tx.clone()) {
            Some(inner) => Self {
                cmd_rx,
                inner: Some(inner),
            },
            None => Self {
                cmd_rx,
                inner: None,
            },
        }
    }

    /// Drain pending OS commands (non-blocking).
    pub fn poll(&self) -> Vec<MediaCommand> {
        let mut out = Vec::new();
        loop {
            match self.cmd_rx.try_recv() {
                Ok(cmd) => out.push(cmd),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Push the current track / status to the OS.
    pub fn publish(&mut self, np: &MediaNowPlaying) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use souvlaki::{MediaMetadata, MediaPlayback, MediaPosition};
            let playback = match np.status {
                PlayerStatus::Playing => MediaPlayback::Playing {
                    progress: Some(MediaPosition(np.position)),
                },
                PlayerStatus::Paused => MediaPlayback::Paused {
                    progress: Some(MediaPosition(np.position)),
                },
                PlayerStatus::Stopped => MediaPlayback::Stopped,
            };
            let _ = inner.controls.set_playback(playback);
            if let Some(track) = np.track.as_ref() {
                let duration = Duration::from_secs(track.duration_secs);
                let cover = np.cover_url.clone();
                let _ = inner.controls.set_metadata(MediaMetadata {
                    title: Some(&track.title),
                    album: track.album.as_deref(),
                    artist: Some(&track.artist),
                    duration: Some(duration),
                    cover_url: cover.as_deref(),
                });
            }
        }
    }
}

fn attach(tx: Sender<MediaCommand>) -> Option<Inner> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use souvlaki::{MediaControlEvent, MediaControls, PlatformConfig};
        let hwnd = None;
        let config = PlatformConfig {
            dbus_name: "tiders",
            display_name: "Tiders",
            hwnd,
        };
        let mut controls = MediaControls::new(config).ok()?;
        controls
            .attach(move |event| {
                let cmd = match event {
                    MediaControlEvent::Play => MediaCommand::Play,
                    MediaControlEvent::Pause => MediaCommand::Pause,
                    MediaControlEvent::Toggle => MediaCommand::PlayPause,
                    MediaControlEvent::Stop => MediaCommand::Stop,
                    MediaControlEvent::Next => MediaCommand::Next,
                    MediaControlEvent::Previous => MediaCommand::Previous,
                    MediaControlEvent::Seek(dir) => {
                        let seconds = match dir {
                            souvlaki::SeekDirection::Forward => 10.0,
                            souvlaki::SeekDirection::Backward => -10.0,
                        };
                        MediaCommand::SeekBy { seconds }
                    }
                    MediaControlEvent::SeekBy(dir, dur) => {
                        let mut seconds = dur.as_secs_f64();
                        if matches!(dir, souvlaki::SeekDirection::Backward) {
                            seconds = -seconds;
                        }
                        MediaCommand::SeekBy { seconds }
                    }
                    MediaControlEvent::SetPosition(pos) => MediaCommand::SeekTo {
                        seconds: pos.0.as_secs_f64(),
                    },
                    _ => return,
                };
                let _ = tx.send(cmd);
            })
            .ok()?;
        Some(Inner { controls })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = tx;
        None
    }
}
