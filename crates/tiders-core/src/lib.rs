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
//! - [`player`] — a small playback engine with pluggable [`player::AudioBackend`]s
//!   (an `mpv` subprocess backend for real audio on macOS + Linux, and a
//!   headless [`player::NullBackend`] used for tests and no-audio environments).
//! - [`queue`] — the play queue, cursor, shuffle and repeat.
//! - [`spectrum`] — a real FFT analyser (`rustfft`) with cava-style gravity.
//! - [`media`] — OS media controls (MPRIS on Linux, Now Playing on macOS).
//! - [`model`] — trimmed-down view models the front-ends render.
//!
//! The CLI, the TUI, and any future GUI (for example a Noctalia/KWin plugin or a
//! background daemon) are expected to drive the same [`TidalService`] and
//! [`player::Player`] types, so behaviour stays consistent across front-ends.

pub mod config;
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
pub use error::{Error, Result};
pub use lyrics::{current_line_index, parse_lrc, LyricLine};
pub use media::{MediaBridge, MediaCommand, MediaNowPlaying};
pub use player::{AudioBackend, BackendKind, NullBackend, Player, PlayerState, PlayerStatus};
pub use playcount::PlayCountStore;
pub use queue::{Queue, QueueItem, RepeatMode, ShuffleMode};
pub use session::{DeviceLogin, StreamInfo, TidalService};
pub use spectrum::{EqTheme, Spectrum, SpectrumFrame};

// Re-export the underlying TIDAL client library so front-ends can reach the raw
// models (search hits, tracks, albums, …) without adding a direct dependency.
pub use tidlers;
