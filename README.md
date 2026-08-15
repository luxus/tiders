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
  [ratatui](https://ratatui.rs)) with search, library, favorites, a play queue,
  and a now‑playing stage. Visual design and the **120 Hz** frame loop take cues
  from [xai-org/grok-build](https://github.com/xai-org/grok-build).
- **Live filter** — `/` fuzzy-filters the current view (library, mixes, playlists,
  favorites, queue, album tracks) with the SIMD matcher from
  [FFF](https://github.com/dmtrKovalenko/fff) (`neo_frizbee`). Typo-resistant,
  ranked as you type, with match highlighting. On the Search tab, **Enter** still
  queries the TIDAL catalog.
- **Library** — your playlists, **My Mixes**, and TIDAL **For You** cards, plus
  album / artist / playlist drill‑down (bio included).
- **Favorites** — saved tracks, albums, and artists; love / unlove syncs with
  TIDAL (`l`).
- **Now‑playing mode** — press `m` for a large cover, synced lyrics, a mini
  queue, and the spectrum (`Esc` back).
- **Real FFT spectrum** — [rustfft](https://crates.io/crates/rustfft) analyser
  with cava‑style gravity, peak hold, and EQ themes (`e` / `E`). Decoded PCM is
  tapped from a silent mpv `--ao=pcm` sidecar so the bars track the actual
  stream (with a seeded synth fallback before the tap is ready).
- **Stream quality** — live decoder details (FLAC / AAC, bit depth, kHz, kbps)
  from TIDAL’s manifest plus mpv’s `audio-params`.
- **Shuffle & repeat** — shuffle off / random / favourites / discovery (`s`);
  repeat off / all / one (`r`). Favourites and discovery are weighted by a
  local play‑count store.
- **Queue** — add a track (`a`) or the whole view (`A`) without interrupting
  playback; the queue persists across restarts. Track radio with `R`.
- **OS media controls** — **MPRIS** on Linux (`playerctl`, GNOME/KDE applets,
  Noctalia) and **Now Playing / media keys** on macOS (via a long‑lived mpv
  plus the platform media session).
- **Toasts with album art** — fade‑out status toasts carry the current cover.
- **Inline album art** — cover art rendered via
  [ratatui-image](https://crates.io/crates/ratatui-image): kitty/sixel/iTerm2,
  with a unicode half‑block fallback.
- **Smooth animation** — time‑based easing (not tick-counted frames) for
  popups, the now‑playing layout, marquee, and toasts. The loop targets **120 Hz**
  while something is moving (playback, spectrum, toasts) and parks when idle,
  with terminal synchronized updates so frames don't tear. Input runs on its own
  OS thread so the UI never blocks on mpv IPC.
- **Scriptable CLI** — `login`, `search`, `play`, `favorites`, `playlists`,
  `whoami`, `logout`.
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
[`PlayerState`] and a self‑contained `Player`, so a future **daemon** (and, for
example, a **Noctalia/KWin plugin** or an MPRIS bridge) can drive playback
through the exact same types without duplicating logic. The `mpv` process model
is the same seam the desktop version would use.

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

If you are not signed in, the TUI shows a **device‑login** screen with a URL and
a code — open the URL, enter the code, approve, and it drops you into your
library. Keys:

| Key | Action |
|-----|--------|
| `/` | live-filter the current list (playlists, mixes, favorites, queue, …). On Search, **Enter** also queries the TIDAL catalog |
| `Tab` / `1` `2` `3` `4` | Search · Library · Favorites · Queue |
| `t` | cycle search scope (tracks / albums / artists / playlists) |
| `S` | cycle Library or Favorites section |
| `↑`/`↓` or `k`/`j` | move selection |
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
| `m` | now-playing mode (big cover, lyrics, mini queue) |
| `e` / `E` | toggle spectrum / cycle EQ theme |
| `d` | track details (cover, BPM, stream quality) |
| `Q` | change audio quality |
| `f` | reload library |
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

`<config>` is `~/.config` on Linux and `~/Library/Application Support` on macOS,
overridable with `--config-dir` or the `TIDERS_CONFIG_DIR` environment variable.

Album art auto‑detects the terminal's image protocol; force one with
`TIDERS_IMAGE_PROTOCOL=halfblocks|sixel|kitty|iterm2` (half‑blocks works
everywhere).

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
   **GitHub Release** with those binaries.

This does **not** run `cargo publish`. To allow the release PR, enable
**Allow GitHub Actions to create and approve pull requests** under
Settings → Actions → General → Workflow permissions.

## Roadmap

- Hi‑Res DASH assembly (segment stitching) for the `hires` tier.
- A background **daemon** with an IPC control socket, so GUI front‑ends such as
  a **Noctalia** plugin can drive the same engine (MPRIS is already in-process).

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
