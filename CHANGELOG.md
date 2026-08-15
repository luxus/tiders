# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

- Spectrum visualiser PCM tap is locked to the main player's clock instead of
  wall-time from when the silent sidecar started, so bars match the audible
  position (including seek / pause).
