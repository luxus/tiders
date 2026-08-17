# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/luxus/tiders/releases/tag/v0.1.0) - 2026-08-17

### Added

- *(tui)* Mixes tab, mouse, toast/scrub fixes, slower redraw
- *(tui)* inline album art, popups (help/detail/quality), progress bar, marquee, gradient spectrum, toasts, animations
- *(app)* tiders CLI + ratatui TUI, mpv playback, Cloud Agent environment, docs
- *(core)* tiders-core library — config, session/auth (tidlers), player engine, queue

### Fixed

- *(player)* keep the spectrum on the same clock as the speakers ([#10](https://github.com/luxus/tiders/pull/10))
- spectrum, context menu, full library, and toast position ([#7](https://github.com/luxus/tiders/pull/7))
- *(tui)* address review notes on favorites, artist errors, and filter hits
- *(tui)* decay spectrum, pad cover art, and official-app layout
- *(tui)* write Now Playing covers with create_new
- *(tui)* map the last scrubber cell to the end of the track
- *(tui)* drop lazy or_else when the fallback is None

### Other

- Engine IPC daemon, Hi-Res DASH assembly, and playlist download ([#8](https://github.com/luxus/tiders/pull/8))
- Player features: mixes, OS media, FFT spectrum, now-playing, 120Hz ([#3](https://github.com/luxus/tiders/pull/3))

### Added

- Background `tiders daemon` with a versioned Unix JSON IPC socket so GUI
  front-ends (Noctalia, later Sendspin / Music Assistant adapters) drive the
  same engine. `tiders ctl` is the CLI client. Hello frames advertise
  Sendspin-shaped roles (`controller`, `metadata`, `artwork`, `visualizer`,
  `player`) without implementing those protocols yet.
- Hi-Res DASH assembly: playback writes a static MPD with absolute segment
  URLs; downloads stitch init + media fragments and remux to FLAC when
  `ffmpeg` is available.
- `tiders download playlist|track|album` for offline copies.

### Fixed

- Spectrum visualiser taps `showfreqs` from the **same** mpv that plays to the
  speakers (`asplit` + `video-sync=audio`) instead of a second `--ao=pcm`
  decoder. The extra process could not stay on the audible clock, so bars
  drifted from the music after buffering, pause, or seek.
- Direct and DASH downloads stream HTTP bodies to a sibling `.part` file
  instead of buffering the whole track in memory.
- `DownloadPlaylist` errors on an empty playlist, matching `PlayPlaylist`.
- IPC `bind()` surfaces socket-directory errors and sets the control socket
  to mode `0600`.
