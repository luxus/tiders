//! Where Tiders stores its session and settings.
//!
//! Locations follow platform conventions via the [`dirs`] crate:
//!
//! | Platform | Config dir |
//! |----------|-----------|
//! | Linux    | `$XDG_CONFIG_HOME/tiders` (usually `~/.config/tiders`) |
//! | macOS    | `~/Library/Application Support/tiders` |
//!
//! The `TIDERS_CONFIG_DIR` environment variable overrides the directory, which
//! keeps tests hermetic and lets power users relocate their session.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::player::BackendKind;
use crate::queue::{RepeatMode, ShuffleMode};
use crate::spectrum::EqTheme;

/// Environment variable that overrides the config directory.
pub const CONFIG_DIR_ENV: &str = "TIDERS_CONFIG_DIR";

/// Application folder name used under the platform config directory.
pub const APP_DIR: &str = "tiders";

/// Audio quality tiers exposed by the app, mirroring TIDAL's own ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Quality {
    Low,
    High,
    Lossless,
    HiRes,
}

impl Quality {
    /// Map to the `tidlers` audio-quality enum.
    pub fn to_api(self) -> tidlers::client::models::playback::AudioQuality {
        use tidlers::client::models::playback::AudioQuality;
        match self {
            Quality::Low => AudioQuality::Low,
            Quality::High => AudioQuality::High,
            Quality::Lossless => AudioQuality::Lossless,
            Quality::HiRes => AudioQuality::HiRes,
        }
    }

    /// Short, human-friendly label.
    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => "Low · AAC 96 kbps",
            Quality::High => "High · AAC 320 kbps",
            Quality::Lossless => "Lossless · FLAC 16-bit 44.1 kHz",
            Quality::HiRes => "Hi-Res · FLAC up to 24-bit 192 kHz",
        }
    }

    /// Compact label for chrome / headers.
    pub fn short_label(self) -> &'static str {
        match self {
            Quality::Low => "Low",
            Quality::High => "High",
            Quality::Lossless => "Lossless",
            Quality::HiRes => "Hi-Res",
        }
    }
}

impl std::str::FromStr for Quality {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Quality::Low),
            "high" => Ok(Quality::High),
            "lossless" | "flac" => Ok(Quality::Lossless),
            "hires" | "hi-res" | "master" | "max" => Ok(Quality::HiRes),
            other => Err(Error::other(format!("unknown quality: {other}"))),
        }
    }
}

/// ReplayGain mode passed through to mpv.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGain {
    #[default]
    Album,
    Track,
    Off,
}

impl ReplayGain {
    pub fn mpv_flag(self) -> &'static str {
        match self {
            ReplayGain::Album => "album",
            ReplayGain::Track => "track",
            ReplayGain::Off => "no",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ReplayGain::Album => "album",
            ReplayGain::Track => "track",
            ReplayGain::Off => "off",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            ReplayGain::Album => ReplayGain::Track,
            ReplayGain::Track => ReplayGain::Off,
            ReplayGain::Off => ReplayGain::Album,
        }
    }
}

/// User-tweakable settings, persisted next to the session as `settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Preferred streaming quality.
    pub quality: Quality,
    /// Startup playback volume (0–100).
    pub volume: u8,
    /// Which audio backend to use.
    pub backend: BackendKind,
    pub shuffle: ShuffleMode,
    pub repeat: RepeatMode,
    pub replaygain: ReplayGain,
    pub eq_theme: EqTheme,
    pub show_spectrum: bool,
    /// Crossfade length in seconds (0 = gapless only).
    pub crossfade_secs: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quality: Quality::Lossless,
            volume: 90,
            backend: BackendKind::Auto,
            shuffle: ShuffleMode::Off,
            repeat: RepeatMode::Off,
            replaygain: ReplayGain::Album,
            eq_theme: EqTheme::Tide,
            show_spectrum: true,
            crossfade_secs: 0,
        }
    }
}

/// Resolved on-disk locations for the current run.
#[derive(Debug, Clone)]
pub struct Config {
    dir: PathBuf,
}

impl Config {
    /// Resolve the config directory, honouring [`CONFIG_DIR_ENV`].
    pub fn resolve() -> Result<Self> {
        let dir = if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV) {
            PathBuf::from(dir)
        } else {
            dirs::config_dir()
                .ok_or_else(|| Error::other("could not determine a config directory"))?
                .join(APP_DIR)
        };
        Ok(Self { dir })
    }

    /// Construct a config rooted at an explicit directory (used by tests).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The root config directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path to the persisted TIDAL session (tokens).
    pub fn session_path(&self) -> PathBuf {
        self.dir.join("session.json")
    }

    /// Path to the persisted settings.
    pub fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    /// Path to the local play-count store.
    pub fn playcounts_path(&self) -> PathBuf {
        self.dir.join("playcounts.json")
    }

    /// Path to the persisted play queue.
    pub fn queue_path(&self) -> PathBuf {
        self.dir.join("queue.json")
    }

    /// Directory used for temporary cover-art / DASH manifest files.
    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join("cache")
    }

    /// Ensure the config directory exists.
    pub fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))
    }

    /// Load settings, falling back to defaults when the file is missing.
    pub fn load_settings(&self) -> Result<Settings> {
        let path = self.settings_path();
        match std::fs::read_to_string(&path) {
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    /// Persist settings, creating the directory if needed.
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.ensure_dir()?;
        let path = self.settings_path();
        let raw = serde_json::to_string_pretty(settings)?;
        std::fs::write(&path, raw).map_err(|e| Error::io(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_roundtrips_through_str() {
        for (input, expected) in [
            ("low", Quality::Low),
            ("HIGH", Quality::High),
            ("flac", Quality::Lossless),
            ("hi-res", Quality::HiRes),
            ("master", Quality::HiRes),
        ] {
            assert_eq!(input.parse::<Quality>().unwrap(), expected);
        }
        assert!("nonsense".parse::<Quality>().is_err());
    }

    #[test]
    fn paths_are_under_the_root() {
        let cfg = Config::at("/tmp/tiders-test-xyz");
        assert!(cfg.session_path().starts_with(cfg.dir()));
        assert!(cfg.settings_path().ends_with("settings.json"));
    }

    #[test]
    fn settings_default_when_missing() {
        let tmp = std::env::temp_dir().join(format!("tiders-cfg-{}", std::process::id()));
        let cfg = Config::at(&tmp);
        let settings = cfg.load_settings().unwrap();
        assert_eq!(settings.quality, Quality::Lossless);
        assert_eq!(settings.volume, 90);
    }
}
