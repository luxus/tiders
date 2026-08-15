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
use crate::model::{StreamQuality, TrackView};
use crate::playcount::PlayCountStore;
use crate::queue::{Queue, RepeatMode, ShuffleMode};

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
        self.build_with(volume, "album")
    }

    pub fn build_with(self, volume: u8, replaygain: &str) -> Result<Box<dyn AudioBackend>> {
        match self {
            BackendKind::Null => Ok(Box::new(NullBackend::new())),
            BackendKind::Mpv => Ok(Box::new(MpvBackend::with_replaygain(volume, replaygain)?)),
            BackendKind::Auto => {
                if MpvBackend::is_available() {
                    Ok(Box::new(MpvBackend::with_replaygain(volume, replaygain)?))
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
    pub shuffle: ShuffleMode,
    pub repeat: RepeatMode,
    pub quality: StreamQuality,
}

/// The playback engine.
pub struct Player {
    backend: Box<dyn AudioBackend>,
    queue: Queue,
    status: PlayerStatus,
    volume: u8,
    now_playing: Option<TrackView>,
    quality: StreamQuality,
    /// Seconds of the current track already counted toward a play.
    listened: f64,
    scrobbled: bool,
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
            quality: StreamQuality::default(),
            listened: 0.0,
            scrobbled: false,
        }
    }

    /// Create a player, building the backend from a [`BackendKind`].
    pub fn with_backend(kind: BackendKind, volume: u8) -> Result<Self> {
        Ok(Self::new(kind.build(volume)?, volume))
    }

    pub fn with_backend_replaygain(
        kind: BackendKind,
        volume: u8,
        replaygain: &str,
    ) -> Result<Self> {
        Ok(Self::new(kind.build_with(volume, replaygain)?, volume))
    }

    /// Name of the active backend (`"mpv"` / `"null"`).
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Immutable access to the queue.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Mutable access to the queue (for persist / shuffle rebuilds).
    pub fn queue_mut(&mut self) -> &mut Queue {
        &mut self.queue
    }

    pub fn status(&self) -> PlayerStatus {
        self.status
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    pub fn now_playing(&self) -> Option<&TrackView> {
        self.now_playing.as_ref()
    }

    pub fn quality(&self) -> &StreamQuality {
        &self.quality
    }

    pub fn shuffle(&self) -> ShuffleMode {
        self.queue.shuffle()
    }

    pub fn repeat(&self) -> RepeatMode {
        self.queue.repeat()
    }

    pub fn set_queue(&mut self, items: Vec<TrackView>, start: usize) {
        self.queue.replace(items, start);
    }

    pub fn enqueue(&mut self, item: TrackView) {
        self.queue.push(item);
    }

    pub fn enqueue_all(&mut self, items: Vec<TrackView>) {
        self.queue.extend(items);
    }

    pub fn cycle_shuffle(&mut self, counts: Option<&PlayCountStore>) -> ShuffleMode {
        let next = self.queue.shuffle().cycle();
        self.queue.apply_shuffle(next, counts);
        next
    }

    pub fn set_shuffle(&mut self, mode: ShuffleMode, counts: Option<&PlayCountStore>) {
        self.queue.apply_shuffle(mode, counts);
    }

    pub fn cycle_repeat(&mut self) -> RepeatMode {
        let next = self.queue.repeat().cycle();
        self.queue.set_repeat(next);
        let _ = self.backend.set_loop_file(next == RepeatMode::One);
        next
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.queue.set_repeat(mode);
        let _ = self.backend.set_loop_file(mode == RepeatMode::One);
    }

    pub fn select(&mut self, index: usize) -> Option<TrackView> {
        self.queue.set_cursor(index).cloned()
    }

    pub fn current(&self) -> Option<&TrackView> {
        self.queue.current()
    }

    /// Begin playing the current queue item from a resolved stream `url`.
    pub fn play_current(&mut self, url: &str) -> Result<()> {
        self.play_current_with_quality(url, StreamQuality::default())
    }

    pub fn play_current_with_quality(&mut self, url: &str, quality: StreamQuality) -> Result<()> {
        let Some(track) = self.queue.current().cloned() else {
            return Ok(());
        };
        self.backend.set_volume(self.volume)?;
        self.backend.play(url)?;
        let _ = self.backend.set_media_title(&track.title);
        let _ = self
            .backend
            .set_loop_file(self.queue.repeat() == RepeatMode::One);
        self.now_playing = Some(track);
        self.status = PlayerStatus::Playing;
        self.quality = quality;
        self.listened = 0.0;
        self.scrobbled = false;
        Ok(())
    }

    /// Merge decoder-reported params into the advertised stream quality.
    pub fn refresh_quality(&mut self) {
        let decoded = self.backend.stream_quality();
        if decoded.sample_rate_hz.is_some() {
            self.quality.sample_rate_hz = decoded.sample_rate_hz;
        }
        if decoded.bit_depth.is_some() {
            self.quality.bit_depth = decoded.bit_depth;
        }
        if decoded.channels.is_some() {
            self.quality.channels = decoded.channels;
        }
        if decoded.bitrate_bps.is_some() {
            self.quality.bitrate_bps = decoded.bitrate_bps;
        }
        if self.quality.codecs.is_none() {
            self.quality.codecs = decoded.codecs;
        }
    }

    pub fn toggle_pause(&mut self) -> Result<()> {
        match self.status {
            PlayerStatus::Playing => self.pause(),
            PlayerStatus::Paused => self.resume(),
            PlayerStatus::Stopped => Ok(()),
        }
    }

    pub fn pause(&mut self) -> Result<()> {
        if self.status == PlayerStatus::Playing {
            self.backend.pause()?;
            self.status = PlayerStatus::Paused;
        }
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if self.status == PlayerStatus::Paused {
            self.backend.resume()?;
            self.status = PlayerStatus::Playing;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.backend.stop()?;
        self.status = PlayerStatus::Stopped;
        self.now_playing = None;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.volume = volume.min(100);
        self.backend.set_volume(self.volume)?;
        Ok(())
    }

    pub fn volume_up(&mut self, step: u8) -> Result<()> {
        let v = self.volume.saturating_add(step).min(100);
        self.set_volume(v)
    }

    pub fn volume_down(&mut self, step: u8) -> Result<()> {
        let v = self.volume.saturating_sub(step);
        self.set_volume(v)
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        self.backend.seek(seconds.max(0.0))
    }

    pub fn seek_by(&mut self, delta: f64) -> Result<()> {
        let pos = self.position().unwrap_or(0.0);
        self.seek((pos + delta).max(0.0))
    }

    pub fn position(&mut self) -> Option<f64> {
        self.backend.position()
    }

    pub fn duration(&mut self) -> Option<f64> {
        self.backend
            .duration()
            .or_else(|| self.now_playing.as_ref().map(|t| t.duration_secs as f64))
    }

    /// Drain decoded PCM from the backend (empty if no tap is available).
    pub fn drain_pcm(&mut self) -> Vec<f32> {
        let mut dst = Vec::new();
        self.backend.drain_pcm(&mut dst);
        dst
    }

    pub fn next_track(&mut self) -> Option<TrackView> {
        self.queue.advance().cloned()
    }

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
            if self.queue.repeat() == RepeatMode::One {
                // mpv loop-file should have restarted; treat as not finished.
                return false;
            }
            if !self.queue.has_next() {
                self.status = PlayerStatus::Stopped;
                self.now_playing = None;
            }
            return true;
        }
        false
    }

    /// Record listening time; bump play counts once past 50% of the track.
    pub fn note_progress(&mut self, dt: f64, counts: Option<&mut PlayCountStore>) {
        if self.status != PlayerStatus::Playing {
            return;
        }
        self.listened += dt.max(0.0);
        if self.scrobbled {
            return;
        }
        let dur = self
            .now_playing
            .as_ref()
            .map(|t| t.duration_secs as f64)
            .unwrap_or(0.0);
        let threshold = if dur > 0.0 { dur * 0.5 } else { 240.0 };
        if self.listened >= threshold {
            if let (Some(store), Some(track)) = (counts, self.now_playing.clone()) {
                store.bump(&track);
            }
            self.scrobbled = true;
        }
    }

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
            shuffle: self.queue.shuffle(),
            repeat: self.queue.repeat(),
            quality: self.quality.clone(),
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
            ..TrackView::default()
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
        let json = serde_json::to_string(&p.snapshot()).unwrap();
        assert!(json.contains("\"status\":\"playing\""));
        assert!(json.contains("\"shuffle\":\"off\""));
    }

    #[test]
    fn note_progress_bumps_playcount_at_halfway() {
        let dir = std::env::temp_dir().join(format!("tiders-pc-player-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("playcounts.json");
        let _ = std::fs::remove_file(&path);
        let mut store = PlayCountStore::load(&path).unwrap();
        let mut p = null_player();
        p.set_queue(vec![track(1)], 0);
        p.play_current("u").unwrap();
        p.note_progress(50.0, Some(&mut store));
        assert_eq!(store.get(&track(1)), 0);
        p.note_progress(60.0, Some(&mut store));
        assert_eq!(store.get(&track(1)), 1);
        let _ = std::fs::remove_file(&path);
    }
}
