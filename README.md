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
  [ratatui](https://ratatui.rs)) with search, favorites, a play queue, and a
  now‑playing bar. Visual design takes cues from
  [xai-org/grok-build](https://github.com/xai-org/grok-build).
- **Inline album art** — cover art rendered right in the terminal via
  [ratatui-image](https://crates.io/crates/ratatui-image): the native graphics
  protocol on kitty/sixel/iTerm2, with a **unicode half‑block** fallback that
  works in any truecolor terminal.
- **Modals & animation** — a Help overlay, a Track‑Details popup (with cover
  art), and a Quality selector, all with an open animation; plus a scrolling
  marquee title, a gradient spectrum, a live progress bar, fade‑out toasts, and
  loading spinners.
- **Scriptable CLI** — `login`, `search`, `play`, `favorites`, `playlists`,
  `whoami`, `logout` for pipelines and quick one‑offs.
- **Secure device‑code login** — the standard TIDAL OAuth device flow; the
  session token is stored under your platform config dir and refreshed
  automatically.
- **Real streaming** — resolves per‑track stream URLs at your chosen quality
  (Low / High / Lossless / Hi‑Res) and plays them via `mpv`.
- **Queue + playback controls** — play/pause, next/prev, volume, and automatic
  advance to the next track.
- **Cross‑platform** — macOS and Linux (KWin/Noctalia, GNOME, COSMIC, …); no
  desktop environment required.

## Architecture

Tiders is a small Cargo workspace split so that the same engine backs every
front‑end:

```
crates/
├── tiders-core/   # front-end-agnostic engine (library)
│   ├── config     # where the session + settings live (XDG / ~/Library)
│   ├── session    # OAuth device login, persistence, catalog access (tidlers)
│   ├── model      # trimmed view models (tracks, albums, artists, playlists)
│   ├── queue      # the play queue + cursor
│   ├── player     # playback engine with pluggable AudioBackend
│   │   ├── mpv    #   → drives an out-of-process mpv over its JSON IPC socket
│   │   └── null   #   → headless backend for tests / no-audio environments
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

Tagged versions (`vX.Y.Z`) publish Linux and macOS binaries via GitHub Actions.
Grab the archive for your platform from
[Releases](https://github.com/luxus/tiders/releases) and unpack the `tiders`
binary onto your `PATH`.

```sh
# after extracting, e.g.
mkdir -p ~/.local/bin
install -m 755 tiders ~/.local/bin/tiders
```

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

### Build

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
| `/` | search |
| `Tab` / `1` `2` `3` | switch Search / Favorites / Queue |
| `↑`/`↓` or `k`/`j` | move selection |
| `Enter` | play selected (seeds the queue) |
| `Space` | play / pause |
| `n` / `p` | next / previous |
| `+` / `-` | volume |
| `d` | track details (with cover art) |
| `Q` | change audio quality |
| `f` | (re)load favorites |
| `s` | stop |
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
| `<config>/tiders/settings.json` | quality, volume, backend |

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

## Roadmap

- Hi‑Res DASH assembly (segment stitching) for the `hires` tier.
- Album/artist/playlist drill‑down and track radio in the TUI.
- A background **daemon** with an IPC control socket + MPRIS, enabling GUI
  front‑ends such as a **Noctalia** plugin to drive the same engine.
- Lyrics and play history (as in Maré Player).

## Acknowledgements

- [Maré Player](https://github.com/glima/mare-player) — the COSMIC app this ports.
- [tidlers](https://codeberg.org/tomkoid/tidlers) — the TIDAL API client.
- [ratatui](https://ratatui.rs) and [xai-org/grok-build](https://github.com/xai-org/grok-build)
  — TUI framework and design inspiration.
- [mpv](https://mpv.io) — the playback engine.

## License

[MIT](LICENSE)
