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
    /// Cover URL. On macOS this must be a `file://` path — HTTP is ignored by
    /// `NSImage` in a CLI app (App Transport Security) and also races if we
    /// republish metadata every tick.
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
    last_track_id: Option<u64>,
    last_cover: Option<String>,
    last_status: Option<PlayerStatus>,
}

struct Inner {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    controls: souvlaki::MediaControls,
}

impl MediaBridge {
    /// Attach to the session bus / Now Playing. Returns a dormant bridge when
    /// the platform rejects us (no D-Bus, no AppDelegate, …).
    pub fn start() -> Self {
        #[cfg(target_os = "macos")]
        macos::ensure_app();

        let (cmd_tx, cmd_rx) = mpsc::channel();
        match attach(cmd_tx) {
            Some(inner) => Self {
                cmd_rx,
                inner: Some(inner),
                last_track_id: None,
                last_cover: None,
                last_status: None,
            },
            None => Self {
                cmd_rx,
                inner: None,
                last_track_id: None,
                last_cover: None,
                last_status: None,
            },
        }
    }

    /// Drain pending OS commands (non-blocking).
    pub fn poll(&self) -> Vec<MediaCommand> {
        #[cfg(target_os = "macos")]
        macos::pump();
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
    ///
    /// Metadata (title, artist, cover) is only sent when the track or cover
    /// changes. Republishing it every few hundred milliseconds resets macOS
    /// Now Playing (title collapses, artwork fetch is cancelled, next/prev
    /// handlers look like they belong to `mpv`).
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

            let track_id = np.track.as_ref().map(|t| t.id);
            let cover = np.cover_url.as_deref();
            let meta_changed = track_id != self.last_track_id
                || cover != self.last_cover.as_deref()
                || self.last_status == Some(PlayerStatus::Stopped);
            if meta_changed {
                if let Some(track) = np.track.as_ref() {
                    let duration = Duration::from_secs(track.duration_secs);
                    let cover_owned = np.cover_url.clone();
                    let _ = inner.controls.set_metadata(MediaMetadata {
                        title: Some(&track.title),
                        album: track.album.as_deref(),
                        artist: Some(&track.artist),
                        duration: Some(duration),
                        cover_url: cover_owned.as_deref(),
                    });
                    self.last_cover = cover_owned;
                }
                self.last_track_id = track_id;
            }
            self.last_status = Some(np.status);
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = inner;
            let _ = np;
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

/// Convert a local image path into a `file://` URL for macOS `NSImage`.
pub fn file_url(path: &std::path::Path) -> Option<String> {
    let abs = path
        .canonicalize()
        .ok()
        .unwrap_or_else(|| path.to_path_buf());
    let s = abs.to_str()?;
    if s.starts_with('/') {
        Some(format!("file://{s}"))
    } else {
        Some(format!("file:///{s}"))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::atomic::{AtomicBool, Ordering};

    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    static STARTED: AtomicBool = AtomicBool::new(false);

    /// `NSApplication` must live on the **main** thread. A background runloop
    /// would own a different app instance than `MPRemoteCommandCenter`, which is
    /// why next/prev used to no-op while play/pause (routed via mpv) still worked.
    pub fn ensure_app() {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            // NSApplicationActivationPolicyAccessory — no Dock icon.
            let _: BOOL = msg_send![app, setActivationPolicy: 1isize];
            let _: () = msg_send![app, finishLaunching];
        }
    }

    /// Drain AppKit events and spin the CFRunLoop so Control Center next/prev
    /// handlers actually fire. Non-blocking (`distantPast`).
    pub fn pump() {
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let distant_past: *mut Object = msg_send![class!(NSDate), distantPast];
            let mode: *mut Object = msg_send![
                class!(NSString),
                stringWithUTF8String: c"kCFRunLoopDefaultMode".as_ptr()
            ];
            loop {
                let event: *mut Object = msg_send![
                    app,
                    nextEventMatchingMask: !0usize
                    untilDate: distant_past
                    inMode: mode
                    dequeue: YES
                ];
                if event.is_null() {
                    break;
                }
                let _: () = msg_send![app, sendEvent: event];
            }
            let rl: *mut Object = msg_send![class!(NSRunLoop), currentRunLoop];
            let _: BOOL = msg_send![rl, runMode: mode beforeDate: distant_past];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_url_uses_file_scheme() {
        let dir = std::env::temp_dir();
        let url = file_url(&dir).expect("temp dir should convert");
        assert!(
            url.starts_with("file://"),
            "expected file:// URL, got {url}"
        );
    }
}
