# ≈ Tiders

**A terminal (TUI + CLI) client for the [TIDAL](https://tidal.com) music
streaming service.**

Tiders is a spiritual port of the [Maré Player](https://github.com/glima/mare-player)
COSMIC desktop applet to the terminal — *"maré"* is Portuguese for **tide**. It
keeps the same idea (browse your TIDAL library, search the catalog, and stream
Hi‑Fi audio) but ships as a fast, keyboard‑driven TUI and a scriptable CLI that
run on **macOS and Linux**.

TIDAL access is provided by the up‑to‑date [`tidlers`](https://codeberg.org/tomkoid/tidlers)
library. Audio is played through [`mpv`](https://mpv.io), which handles every
stream shape TIDAL returns (direct FLAC, DASH, HLS) on both platforms.

> Unofficial project. TIDAL is a trademark of TIDAL Music AS; Tiders is not
> affiliated with or endorsed by TIDAL. A paid TIDAL subscription is required to
> stream. Use in accordance with TIDAL's Terms of Service.

---

## Features

- **Interactive TUI** — a fullscreen, themed terminal UI (built with
  [ratatui](https://ratatui.rs)) with search, library, **Mixes**, favorites, a
  play queue, and a now‑playing stage. Mouse clicks select tabs, sections, and
  rows (a second click plays); drag the scrubber to seek. Animations target
  **120 Hz** while something is moving (toasts, layout morphs) and drop to a
  slower redraw while a track is simply playing, so the terminal does not sit
  on ~30% CPU.
- **Live filter** — `/` fuzzy-filters the current view (library, mixes, playlists,
  favorites, queue, album tracks) with the SIMD matcher from
  [FFF](https://github.com/dmtrKovalenko/fff) (`neo_frizbee`). Typo-resistant,
  ranked as you type, with match highlighting. On the Search tab, **Enter** still
  queries the TIDAL catalog.
- **Library** — saved songs in a sortable table, playlists in a hideable
  sidebar, **My Mixes**, and TIDAL **For You**, plus album / artist / playlist
  drill‑down (artist image, top songs, latest albums, bio).
- **Favorites** — saved tracks, albums, and artists; love / unlove syncs with
  TIDAL (`l`). Right-click a track for artist, album, favorite, like, and
  don't like.
- **Now‑playing** — compact bar with cover padding, spectrum, and **Up Next**;
  press `m` for a large cover, synced lyrics, and a clickable scrollable queue
  (`Esc` back).
- **Real FFT spectrum** — [rustfft](https://crates.io/crates/rustfft) analyser
  with cava‑style gravity and peak hold. Bars are driven from a `showfreqs` tap
  inside the **same** mpv that plays to the speakers (`video-sync=audio`), so
  they stay on the audible clock. They fall to zero on silence, pause, or a
  stale tap.
- **Stream quality** — live decoder details (FLAC / AAC, bit depth, kHz, kbps)
  from TIDAL’s manifest plus mpv’s `audio-params`.
- **Shuffle & repeat** — shuffle off / random / favourites / discovery (`s`);
  repeat off / all / one (`r`). Favourites and discovery are weighted by a
  local play‑count store.
- **Queue** — add a track (`a`) or the whole view (`A`) without interrupting
  playback; the queue persists across restarts. Track radio with `R`.
- **OS media controls** — **MPRIS** on Linux (`playerctl`, GNOME/KDE applets,
  Noctalia) and **Now Playing / media keys** on macOS. Tiders owns the session
  (title, artist, cover, next/prev). mpv is told not to register media keys so
  Control Center does not show a combined “Artist — Title” under **mpv**.
- **Toasts** — now‑playing toasts show title, artist, and cover; settings
  toasts (repeat, shuffle, volume) are text‑only.
- **Inline album art** — cover art rendered via
  [ratatui-image](https://crates.io/crates/ratatui-image): kitty/sixel/iTerm2,
  with a unicode half‑block fallback.
- **Smooth animation** — time‑based easing (not tick-counted frames) for
  popups, the now‑playing layout, marquee, and toasts. The loop parks when idle,
  with terminal synchronized updates so frames don't tear. Input runs on its own
  OS thread so the UI never blocks on mpv IPC.
- **Scriptable CLI** — `login`, `search`, `play`, `favorites`, `playlists`,
  `download`, `daemon` / `ctl`, `whoami`, `logout`.
- **Secure device‑code login** — OAuth device flow; the session is stored under
  your platform config dir and refreshed automatically.
- **Gapless streaming** — a persistent `mpv` (`--idle=yes --gapless-audio=yes`)
  handles FLAC, DASH, and HLS on both platforms.

## Architecture

Tiders is a small Cargo workspace split so that the same engine backs every
front‑end:

```
crates/
├── tiders-core/   # front-end-agnostic engine (library)
│   ├── config     # session + settings (XDG / ~/Library)
│   ├── session    # OAuth device login, catalog, mixes, lyrics
│   ├── model      # trimmed view models
│   ├── queue      # play queue, shuffle, repeat
│   ├── playcount  # local play-count store (weighted shuffle)
│   ├── lyrics     # LRC parser
│   ├── spectrum   # rustfft analyser
│   ├── media      # MPRIS (Linux) / Now Playing (macOS)
│   ├── player     # playback engine with pluggable AudioBackend
│   │   ├── mpv    #   persistent mpv over JSON IPC
│   │   └── null   #   headless backend for tests
│   └── format     # shared formatting helpers
└── tiders/        # the `tiders` binary: clap CLI + ratatui TUI
```

**Why this split?** The GUI comes later. `tiders-core` exposes a serialisable
[`PlayerState`] / [`EngineState`] and a command bus (`EngineCommand` /
`EngineEvent`), so a **daemon**, a **Noctalia** plugin, and later a **Sendspin**
or **Music Assistant** adapter all drive the same session and audio backend.

```
  TUI  CLI  Noctalia plugin         Sendspin / Music Assistant (later)
    \   |   /                       controller · metadata · artwork ·
     EngineCommand / EngineEvent    visualizer · player  (thin adapters)
              |
         tiders engine              (session, queue, DASH stitch, downloads)
              |
    ┌─────────┴──────────┐
    mpv AudioBackend     Unix IPC + MPRIS
    (Sendspin player     $XDG_RUNTIME_DIR/tiders.sock
     sink later)
```

Sendspin and Music Assistant are **not** implemented yet. The IPC hello frame
advertises those role names so a future crate can translate `controller@v1`
messages onto `EngineCommand` and `metadata` / `visualizer` events onto
`EngineEvent` without a second player. Music Assistant already speaks Sendspin
natively, so one Sendspin client adapter covers “Tiders as an MA player or
wall display”. A Sendspin *server* (Tiders sourcing audio to MA speakers) is
a separate `AudioBackend` later.

## Install

### Prerequisites

- **Rust** 1.85+ (the [`tidlers`](https://codeberg.org/tomkoid/tidlers) client is
  edition 2024). `rustup` is recommended; `rust-toolchain.toml` pins `stable`.
- **mpv** for audio playback:
  - Linux (Debian/Ubuntu): `sudo apt install mpv`
  - Linux (Fedora): `sudo dnf install mpv`
  - Linux (Arch): `sudo pacman -S mpv`
  - macOS: `brew install mpv`

`mpv` is only needed at runtime for actual audio; everything else (login,
search, browsing) works without it.

### GitHub Releases

Tagged versions (`vX.Y.Z`) publish Linux (`x86_64` and `aarch64`) and macOS
(`aarch64` and Intel) binaries via GitHub Actions. Grab the archive for your
platform from [Releases](https://github.com/luxus/tiders/releases) and unpack
the `tiders` binary onto your `PATH`.

Tiders is **not** published to crates.io because the TIDAL client (`tidlers`)
is a git dependency.

```sh
# after extracting, e.g.
mkdir -p ~/.local/bin
install -m 755 tiders ~/.local/bin/tiders
```

macOS binaries are unsigned; Gatekeeper may ask you to allow the app on first
run.

### Nix

Needs [Nix](https://nixos.org/download/) with flakes (`nix-command` + `flakes`).
`mpv` is wrapped onto the packaged binary's `PATH`, so you do not need to
install it yourself.

**Run without installing** (builds into the Nix store; nothing is added to your
profile or system):

```sh
# from GitHub — launches the interactive TUI
nix run github:luxus/tiders

# CLI subcommands: everything after `--` is passed to tiders
nix run github:luxus/tiders -- --help
nix run github:luxus/tiders -- login
nix run github:luxus/tiders -- search daft punk
```

From a local checkout of this repo:

```sh
nix run .                 # TUI
nix run . -- --help
nix build                 # ./result/bin/tiders
nix flake check           # build + cargo tests + CLI smoke test
nix develop               # rustc/cargo/clippy/rustfmt + mpv
```

**Install via overlay** (NixOS / home-manager / nix-darwin). Apply the overlay
with the `nixpkgs.overlays` module option — `nixosSystem` does not take an
`overlays` argument:

```nix
# flake.nix
{
  inputs.tiders.url = "github:luxus/tiders";

  outputs = { nixpkgs, tiders, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ({ pkgs, ... }: {
          nixpkgs.overlays = [ tiders.overlays.default ];
          environment.systemPackages = [ pkgs.tiders ];
        })
      ];
    };
  };
}
```

home-manager is the same idea: `nixpkgs.overlays = [ tiders.overlays.default ];`
then `home.packages = [ pkgs.tiders ];`.

### Build from source

```sh
git clone https://github.com/luxus/tiders.git
cd tiders
cargo build --release
# binary at target/release/tiders
```

## Usage

### TUI

```sh
tiders            # launches the interactive UI (default)
# or explicitly:
tiders tui
```

Without installing Tiders, the same TUI is:

```sh
nix run github:luxus/tiders
```

If you are not signed in, the TUI shows a **device‑login** screen with a URL and
a code — open the URL, enter the code, approve, and it drops you into your
library. Keys:

| Key | Action |
|-----|--------|
| `/` | live-filter the current list (playlists, mixes, favorites, queue, …). On Search, **Enter** also queries the TIDAL catalog |
| `Tab` / `1`–`7` | Search · For You · Mixes · Library · Playlists · Favorites · Queue |
| `b` / `\` | show / hide the left sidebar |
| click / drag | sidebar, row, queue, or sort header; second click on a row plays; drag the scrubber to seek; wheel moves the selection |
| `c` / right-click | context menu: go to artist / album, add or remove favorite, like, don't like (`j`/`k` + Enter, or click a row) |
| `t` | cycle search scope (tracks / albums / artists / playlists) |
| `S` | cycle Favorites section |
| `o` | cycle table sort (title · artist · album · time); click a header to sort |
| `↑`/`↓` or `k`/`j` | move selection (queue in now-playing mode) |
| `Enter` | play, or open playlist / mix / album / artist |
| `Esc` | back / close / leave now-playing mode |
| `a` / `A` | add track / add all to queue |
| `Space` | play / pause |
| `n` / `p` | next / previous |
| `←` / `→` | seek ±10s |
| `+` / `-` or `]` / `[` | volume |
| `s` | cycle shuffle (off · random · favourites · discovery) |
| `r` | cycle repeat (off · all · one) |
| `R` | start radio from the focused track |
| `l` | love / unlove |
| `m` | now-playing mode (big cover, lyrics, scrollable queue) |
| `e` / `E` | toggle spectrum / cycle EQ theme |
| `d` | track details (cover, BPM, stream quality) |
| `Q` | change audio quality |
| `f` | reload library, mixes, and For You |
| `x` | stop |
| `?` | help |
| `q` | quit |

### CLI

```sh
tiders login                     # device-code sign-in
tiders whoami                    # who am I?
tiders search daft punk          # search tracks/albums/artists/playlists
tiders favorites --limit 25      # your favorite tracks
tiders playlists                 # your playlists
tiders play 66035607             # stream a track by id (needs mpv)
tiders --quality hires play 12345
tiders download playlist aa692128-2954-4fe1-b5a1-4ede1add485d
tiders download track 66035607 --dest ~/Music/Tiders
tiders daemon                    # background engine (IPC + MPRIS)
tiders ctl play-pause            # drive the daemon (Noctalia / scripts)
tiders logout
```

Global flags: `--quality low|high|lossless|hires` and `--config-dir <DIR>`
(handy for isolated profiles/testing).

## Configuration

| File | Purpose |
|------|---------|
| `<config>/tiders/session.json` | saved TIDAL session (tokens) |
| `<config>/tiders/settings.json` | quality, volume, shuffle, repeat, EQ theme |
| `<config>/tiders/playcounts.json` | local play counts (weighted shuffle) |
| `<config>/tiders/queue.json` | persisted play queue |
| `$XDG_RUNTIME_DIR/tiders.sock` | daemon IPC socket (`TIDERS_SOCK` / `--socket`) |
| `~/Music/Tiders/` | default download directory |

`<config>` is `~/.config` on Linux and `~/Library/Application Support` on macOS,
overridable with `--config-dir` or the `TIDERS_CONFIG_DIR` environment variable.

Album art auto‑detects the terminal's image protocol; force one with
`TIDERS_IMAGE_PROTOCOL=halfblocks|sixel|kitty|iterm2` (half‑blocks works
everywhere).

Set `TIDERS_PCM_VIS=0` to skip the spectrum tap if you want to save a little
CPU; the analyser then stays at rest instead of following the playing track.

For headless/CI use, a full session JSON can be supplied via the
`TIDAL_SESSION_JSON` environment variable; Tiders restores and persists it on
first use. On machines without an audio device (CI, servers), set
`TIDERS_MPV_AO=null` so `mpv` decodes the stream in real time without opening an
output.

## CI and releases

GitHub Actions runs on every pull request and every push to `main`:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` on **Linux and macOS** (unit tests do not need `mpv`)

Releases are automated with [release-plz](https://release-plz.dev) from
[Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, …):

1. Merging to `main` opens a **release PR** that bumps the workspace version and
   updates [`CHANGELOG.md`](CHANGELOG.md).
2. Merging that PR tags `vX.Y.Z`.
3. The existing tag workflow builds Linux/macOS archives and publishes a
   **GitHub Release** with those binaries. The same workflow also runs
   `nix build` / `nix flake check` through
   [cachix/cachix-action](https://github.com/cachix/cachix-action) against the
   `tiders` binary cache (`CACHIX_AUTH_TOKEN`). A missing token does not block
   the GitHub Release.

This does **not** run `cargo publish`. To allow the release PR, enable
**Allow GitHub Actions to create and approve pull requests** under
Settings → Actions → General → Workflow permissions.

## Roadmap

- Sendspin client transport (`controller` / `metadata` / `artwork` /
  `visualizer` / `player` roles) so Music Assistant can discover Tiders, and
  a Sendspin `AudioBackend` if Tiders should source audio to MA speakers.
- A Noctalia plugin that talks to `tiders daemon` over the IPC socket.

The Hi-Res DASH stitcher and the IPC daemon are in this tree: see
`tiders-core::dash`, `tiders-core::engine`, `tiders daemon`, and
`tiders download`.

## Acknowledgements

- [Maré Player](https://github.com/glima/mare-player) — the COSMIC app this ports.
- [low-tide](https://github.com/pauljhdrake/low-tide) — library, mixes, shuffle,
  lyrics, and MPRIS behaviour this release draws from.
- [tidlers](https://codeberg.org/tomkoid/tidlers) — the TIDAL API client.
- [ratatui](https://ratatui.rs) and [xai-org/grok-build](https://github.com/xai-org/grok-build)
  — TUI framework and 120 Hz / animation inspiration.
- [FFF](https://github.com/dmtrKovalenko/fff) / [neo_frizbee](https://crates.io/crates/neo_frizbee)
  — SIMD fuzzy matching for in-list filtering.
- [mpv](https://mpv.io) — the playback engine.
- [rustfft](https://crates.io/crates/rustfft) — the spectrum analyser.

## License

[MIT](LICENSE)
