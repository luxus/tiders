//! # tiders-core
//!
//! Core building blocks for **Tiders**, a terminal (TUI + CLI) client for the
//! TIDAL music streaming service — a spiritual port of the
//! [Maré Player](https://github.com/glima/mare-player) COSMIC applet to the
//! terminal.
//!
//! This crate is intentionally front-end agnostic. It exposes:
//!
//! - [`config`] — where sessions and settings live on disk (XDG on Linux,
//!   `~/Library` on macOS).
//! - [`session`] / [`TidalService`] — authentication (OAuth device-code flow),
//!   session persistence, and high-level catalog access built on
//!   [`tidlers`](https://codeberg.org/tomkoid/tidlers).
//! - [`engine`] — command/event bus plus a Unix JSON IPC transport so a
//!   daemon, Noctalia plugin, or (later) Sendspin / Music Assistant adapter
//!   can drive the same [`player::Player`] the TUI uses.
//! - [`player`] — playback engine with pluggable [`player::AudioBackend`]s
//!   (`mpv` on macOS + Linux, headless [`player::NullBackend`] for tests).
//! - [`dash`] — Hi-Res DASH segment assembly (MPD for playback, stitch for
//!   downloads).
//! - [`download`] — playlist / track offline download.
//! - [`queue`] — the play queue, cursor, shuffle and repeat.
//! - [`spectrum`] — a real FFT analyser (`rustfft`) with cava-style gravity.
//! - [`media`] — OS media controls (MPRIS on Linux, Now Playing on macOS).
//! - [`model`] — trimmed-down view models the front-ends render.
//!
//! The CLI, the TUI, and any future GUI (for example a Noctalia/KWin plugin or a
//! background daemon) are expected to drive the same [`TidalService`] and
//! [`player::Player`] types, so behaviour stays consistent across front-ends.

pub mod config;
pub mod dash;
pub mod download;
pub mod engine;
pub mod error;
pub mod format;
pub mod images;
pub mod lyrics;
pub mod media;
pub mod model;
pub mod playcount;
pub mod player;
pub mod queue;
pub mod session;
pub mod spectrum;

pub use config::Config;
pub use dash::DashAssembly;
pub use download::{DownloadReport, DownloadStatus};
pub use engine::{Engine, EngineCommand, EngineEvent, EngineState};
pub use error::{Error, Result};
pub use lyrics::{current_line_index, from_tidal, parse_lrc, LyricLine};
pub use media::{file_url, MediaBridge, MediaCommand, MediaNowPlaying};
pub use playcount::PlayCountStore;
pub use player::{AudioBackend, BackendKind, NullBackend, Player, PlayerState, PlayerStatus};
pub use queue::{Queue, QueueItem, RepeatMode, ShuffleMode};
pub use session::{DeviceLogin, StreamInfo, TidalService};
pub use spectrum::{EqTheme, Spectrum, SpectrumFrame};

// Re-export the underlying TIDAL client library so front-ends can reach the raw
// models (search hits, tracks, albums, …) without adding a direct dependency.
pub use tidlers;
