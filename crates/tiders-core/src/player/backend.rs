//! The audio-backend abstraction.
//!
//! A backend is the thing that actually turns a stream URL into sound. Keeping
//! it behind a trait means the [`Player`](super::Player) — and everything above
//! it — is identical whether we drive a real `mpv` process, a future in-process
//! decoder, or the headless [`NullBackend`] used by tests.

use crate::error::Result;
use crate::model::StreamQuality;

/// Something that can play a single audio stream URL at a time.
///
/// Implementations must be `Send` so the player can live behind an async task
/// or, later, inside a control daemon.
pub trait AudioBackend: Send {
    /// Start playing `url`, replacing anything currently playing.
    fn play(&mut self, url: &str) -> Result<()>;

    /// Pause playback (no-op if already paused/stopped).
    fn pause(&mut self) -> Result<()>;

    /// Resume playback (no-op if already playing/stopped).
    fn resume(&mut self) -> Result<()>;

    /// Stop playback and release the stream.
    fn stop(&mut self) -> Result<()>;

    /// Set the output volume (0–100).
    fn set_volume(&mut self, volume: u8) -> Result<()>;

    /// Poll whether the current track finished **on its own** (reached EOF).
    ///
    /// Returns `true` exactly once per finished track; callers use it to advance
    /// the queue. A stopped/paused backend returns `false`.
    fn poll_finished(&mut self) -> bool;

    /// Human-readable backend name, e.g. `"mpv"` or `"null"`.
    fn name(&self) -> &'static str;

    /// Seek to an absolute position in seconds.
    fn seek(&mut self, seconds: f64) -> Result<()> {
        let _ = seconds;
        Ok(())
    }

    /// Current playback position in seconds, if known.
    fn position(&mut self) -> Option<f64> {
        None
    }

    /// Decoded duration in seconds, if known.
    fn duration(&mut self) -> Option<f64> {
        None
    }

    /// Decoder-reported stream format (sample rate, bit depth, codec, …).
    fn stream_quality(&mut self) -> StreamQuality {
        StreamQuality::default()
    }

    /// Title shown in the OS Now Playing / mpv media-title.
    fn set_media_title(&mut self, title: &str) -> Result<()> {
        let _ = title;
        Ok(())
    }

    /// Loop the current file (repeat-one).
    fn set_loop_file(&mut self, on: bool) -> Result<()> {
        let _ = on;
        Ok(())
    }

    /// Append recently decoded PCM samples (mono `f32`, ~44.1 kHz) into `dst`.
    ///
    /// Default is a no-op. Prefer [`AudioBackend::drain_fft_bins`] when the
    /// backend taps a same-process spectrum; this remains for synthetic PCM.
    fn drain_pcm(&mut self, dst: &mut Vec<f32>) {
        dst.clear();
    }

    /// Linear frequency-bin levels (`0..=1`) from the playback-clock vis tap.
    fn drain_fft_bins(&mut self, dst: &mut Vec<f32>) {
        dst.clear();
    }
}

/// A backend that tracks state but produces no sound.
///
/// Used in tests and in environments without an audio device. It records the
/// last URL and volume so behaviour can be asserted without hardware.
#[derive(Debug, Default)]
pub struct NullBackend {
    playing: bool,
    paused: bool,
    volume: u8,
    last_url: Option<String>,
    position: f64,
}

impl NullBackend {
    /// Create a new, idle null backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// The most recent URL passed to [`AudioBackend::play`].
    pub fn last_url(&self) -> Option<&str> {
        self.last_url.as_deref()
    }

    /// Whether the backend currently considers itself paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The last volume set on the backend.
    pub fn volume(&self) -> u8 {
        self.volume
    }
}

impl AudioBackend for NullBackend {
    fn play(&mut self, url: &str) -> Result<()> {
        self.playing = true;
        self.paused = false;
        self.last_url = Some(url.to_string());
        self.position = 0.0;
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        if self.playing {
            self.paused = true;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.playing {
            self.paused = false;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.playing = false;
        self.paused = false;
        self.position = 0.0;
        Ok(())
    }

    fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.volume = volume.min(100);
        Ok(())
    }

    fn poll_finished(&mut self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "null"
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        self.position = seconds.max(0.0);
        Ok(())
    }

    fn position(&mut self) -> Option<f64> {
        Some(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_backend_tracks_state() {
        let mut b = NullBackend::new();
        b.play("https://example/stream.flac").unwrap();
        assert_eq!(b.last_url(), Some("https://example/stream.flac"));
        assert!(!b.is_paused());
        b.pause().unwrap();
        assert!(b.is_paused());
        b.resume().unwrap();
        assert!(!b.is_paused());
        b.set_volume(150).unwrap();
        assert_eq!(b.volume(), 100);
        assert!(!b.poll_finished());
        b.stop().unwrap();
    }
}
