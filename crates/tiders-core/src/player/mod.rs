//! The playback engine: a [`Player`] that owns a [`Queue`] and drives an
//! [`AudioBackend`].
//!
//! The player is deliberately synchronous and I/O-light: it never talks to
//! TIDAL. Resolving a track id into a stream URL is the caller's job (see
//! [`TidalService::stream_url`](crate::session::TidalService::stream_url)); the
//! player only starts/stops/pauses whatever URL it is handed and tracks queue
//! position. That separation keeps this logic unit-testable with the
//! [`NullBackend`] and lets a future daemon reuse it verbatim.

mod backend;
mod mpv;

pub use backend::{AudioBackend, NullBackend};
pub use mpv::MpvBackend;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::TrackView;
use crate::queue::Queue;

/// High-level playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerStatus {
    Stopped,
    Playing,
    Paused,
}

/// Which audio backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// Use `mpv` if available, otherwise fall back to the null backend.
    #[default]
    Auto,
    /// Force the `mpv` subprocess backend.
    Mpv,
    /// Force the headless null backend (no audio).
    Null,
}

impl BackendKind {
    /// Build the concrete backend for this kind at the given start volume.
    pub fn build(self, volume: u8) -> Result<Box<dyn AudioBackend>> {
        match self {
            BackendKind::Null => Ok(Box::new(NullBackend::new())),
            BackendKind::Mpv => Ok(Box::new(MpvBackend::new(volume)?)),
            BackendKind::Auto => {
                if MpvBackend::is_available() {
                    Ok(Box::new(MpvBackend::new(volume)?))
                } else {
                    Ok(Box::new(NullBackend::new()))
                }
            }
        }
    }
}

/// A serialisable snapshot of the player, suitable for rendering or IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: PlayerStatus,
    pub volume: u8,
    pub backend: String,
    pub now_playing: Option<TrackView>,
    pub queue_len: usize,
    pub queue_position: Option<usize>,
    pub has_next: bool,
    pub has_previous: bool,
}

/// The playback engine.
pub struct Player {
    backend: Box<dyn AudioBackend>,
    queue: Queue,
    status: PlayerStatus,
    volume: u8,
    now_playing: Option<TrackView>,
}

impl Player {
    /// Create a player over an explicit backend at the given start volume.
    pub fn new(backend: Box<dyn AudioBackend>, volume: u8) -> Self {
        Self {
            backend,
            queue: Queue::new(),
            status: PlayerStatus::Stopped,
            volume: volume.min(100),
            now_playing: None,
        }
    }

    /// Create a player, building the backend from a [`BackendKind`].
    pub fn with_backend(kind: BackendKind, volume: u8) -> Result<Self> {
        Ok(Self::new(kind.build(volume)?, volume))
    }

    /// Name of the active backend (`"mpv"` / `"null"`).
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Immutable access to the queue.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// The current playback status.
    pub fn status(&self) -> PlayerStatus {
        self.status
    }

    /// The current volume (0–100).
    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// The track currently loaded into the backend, if any.
    pub fn now_playing(&self) -> Option<&TrackView> {
        self.now_playing.as_ref()
    }

    /// Replace the queue with `items`, positioned at `start`.
    pub fn set_queue(&mut self, items: Vec<TrackView>, start: usize) {
        self.queue.replace(items, start);
    }

    /// Append a track to the queue.
    pub fn enqueue(&mut self, item: TrackView) {
        self.queue.push(item);
    }

    /// Move the queue cursor to `index` without starting playback; returns the
    /// track the caller should now resolve a URL for.
    pub fn select(&mut self, index: usize) -> Option<TrackView> {
        self.queue.set_cursor(index).cloned()
    }

    /// The current queue item (what a URL should be resolved for).
    pub fn current(&self) -> Option<&TrackView> {
        self.queue.current()
    }

    /// Begin playing the current queue item from a resolved stream `url`.
    pub fn play_current(&mut self, url: &str) -> Result<()> {
        let Some(track) = self.queue.current().cloned() else {
            return Ok(());
        };
        self.backend.set_volume(self.volume)?;
        self.backend.play(url)?;
        self.now_playing = Some(track);
        self.status = PlayerStatus::Playing;
        Ok(())
    }

    /// Toggle between playing and paused.
    pub fn toggle_pause(&mut self) -> Result<()> {
        match self.status {
            PlayerStatus::Playing => self.pause(),
            PlayerStatus::Paused => self.resume(),
            PlayerStatus::Stopped => Ok(()),
        }
    }

    /// Pause playback.
    pub fn pause(&mut self) -> Result<()> {
        if self.status == PlayerStatus::Playing {
            self.backend.pause()?;
            self.status = PlayerStatus::Paused;
        }
        Ok(())
    }

    /// Resume playback.
    pub fn resume(&mut self) -> Result<()> {
        if self.status == PlayerStatus::Paused {
            self.backend.resume()?;
            self.status = PlayerStatus::Playing;
        }
        Ok(())
    }

    /// Stop playback and clear the now-playing track.
    pub fn stop(&mut self) -> Result<()> {
        self.backend.stop()?;
        self.status = PlayerStatus::Stopped;
        self.now_playing = None;
        Ok(())
    }

    /// Set the volume (0–100) and push it to the backend.
    pub fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.volume = volume.min(100);
        self.backend.set_volume(self.volume)?;
        Ok(())
    }

    /// Raise the volume by `step`, saturating at 100.
    pub fn volume_up(&mut self, step: u8) -> Result<()> {
        let v = self.volume.saturating_add(step).min(100);
        self.set_volume(v)
    }

    /// Lower the volume by `step`, saturating at 0.
    pub fn volume_down(&mut self, step: u8) -> Result<()> {
        let v = self.volume.saturating_sub(step);
        self.set_volume(v)
    }

    /// Advance the queue cursor and return the next track to resolve, if any.
    pub fn next_track(&mut self) -> Option<TrackView> {
        self.queue.advance().cloned()
    }

    /// Step the queue cursor back and return the previous track to resolve.
    pub fn previous_track(&mut self) -> Option<TrackView> {
        self.queue.previous().cloned()
    }

    /// Poll the backend for track completion.
    ///
    /// Returns `true` when the current track just finished on its own, so the
    /// caller can advance the queue and resolve the next URL. When the queue is
    /// exhausted this also transitions the player to [`PlayerStatus::Stopped`].
    pub fn poll_finished(&mut self) -> bool {
        if self.status != PlayerStatus::Playing {
            return false;
        }
        if self.backend.poll_finished() {
            if !self.queue.has_next() {
                self.status = PlayerStatus::Stopped;
                self.now_playing = None;
            }
            return true;
        }
        false
    }

    /// Build a serialisable snapshot of the current state.
    pub fn snapshot(&self) -> PlayerState {
        PlayerState {
            status: self.status,
            volume: self.volume,
            backend: self.backend.name().to_string(),
            now_playing: self.now_playing.clone(),
            queue_len: self.queue.len(),
            queue_position: self.queue.cursor(),
            has_next: self.queue.has_next(),
            has_previous: self.queue.has_previous(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: u64) -> TrackView {
        TrackView {
            id,
            title: format!("track {id}"),
            artist: "Artist".into(),
            album: Some("Album".into()),
            duration_secs: 200,
            explicit: false,
            cover: None,
        }
    }

    fn null_player() -> Player {
        Player::new(Box::new(NullBackend::new()), 80)
    }

    #[test]
    fn play_pause_resume_stop_state_machine() {
        let mut p = null_player();
        p.set_queue(vec![track(1), track(2)], 0);
        assert_eq!(p.status(), PlayerStatus::Stopped);

        p.play_current("https://example/1.flac").unwrap();
        assert_eq!(p.status(), PlayerStatus::Playing);
        assert_eq!(p.now_playing().map(|t| t.id), Some(1));

        p.toggle_pause().unwrap();
        assert_eq!(p.status(), PlayerStatus::Paused);
        p.toggle_pause().unwrap();
        assert_eq!(p.status(), PlayerStatus::Playing);

        p.stop().unwrap();
        assert_eq!(p.status(), PlayerStatus::Stopped);
        assert!(p.now_playing().is_none());
    }

    #[test]
    fn volume_clamps_and_steps() {
        let mut p = null_player();
        p.set_volume(50).unwrap();
        assert_eq!(p.volume(), 50);
        p.volume_up(60).unwrap();
        assert_eq!(p.volume(), 100);
        p.volume_down(30).unwrap();
        assert_eq!(p.volume(), 70);
        p.volume_down(200).unwrap();
        assert_eq!(p.volume(), 0);
    }

    #[test]
    fn next_and_previous_walk_queue() {
        let mut p = null_player();
        p.set_queue(vec![track(1), track(2), track(3)], 0);
        assert_eq!(p.current().map(|t| t.id), Some(1));
        assert_eq!(p.next_track().map(|t| t.id), Some(2));
        assert_eq!(p.next_track().map(|t| t.id), Some(3));
        assert!(p.next_track().is_none());
        assert_eq!(p.previous_track().map(|t| t.id), Some(2));
    }

    #[test]
    fn snapshot_reflects_state() {
        let mut p = null_player();
        p.set_queue(vec![track(1), track(2)], 0);
        p.play_current("u").unwrap();
        let snap = p.snapshot();
        assert_eq!(snap.status, PlayerStatus::Playing);
        assert_eq!(snap.queue_len, 2);
        assert_eq!(snap.queue_position, Some(0));
        assert!(snap.has_next);
        assert!(!snap.has_previous);
        assert_eq!(snap.now_playing.map(|t| t.id), Some(1));
        // snapshot round-trips through JSON (used for the future daemon/IPC)
        let json = serde_json::to_string(&p.snapshot()).unwrap();
        assert!(json.contains("\"status\":\"playing\""));
    }
}
