//! Command-line interface: argument parsing and command dispatch.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use tokio::sync::mpsc::unbounded_channel;

use tiders_core::config::{Config, Quality, Settings};
use tiders_core::download::{self, DownloadStatus};
use tiders_core::engine::ipc;
use tiders_core::engine::EngineCommand;
use tiders_core::tidlers::client::oauth::OAuthStatus;
use tiders_core::{Player, PlayerStatus, TidalService};

use crate::daemon;
use crate::output;
use crate::tui;

/// Tiders — a terminal (TUI + CLI) client for TIDAL.
#[derive(Debug, Parser)]
#[command(name = "tiders", version, about, long_about = None)]
pub struct Cli {
    /// Override the config/session directory.
    #[arg(long, global = true, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    /// Streaming quality: low | high | lossless | hires.
    #[arg(long, short, global = true, value_name = "QUALITY")]
    quality: Option<Quality>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sign in to TIDAL using the OAuth device-code flow.
    Login,
    /// Sign out and delete the saved session.
    Logout,
    /// Show the currently signed-in account.
    Whoami,
    /// Search the catalog for tracks, albums, artists and playlists.
    Search {
        /// Words to search for.
        #[arg(required = true, num_args = 1.., value_name = "QUERY")]
        query: Vec<String>,
        /// Maximum number of hits per section.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Stream a track by id (needs `mpv` for audio).
    Play {
        /// Numeric TIDAL track id.
        track_id: u64,
    },
    /// List your favorite tracks.
    Favorites {
        /// Maximum number of tracks to list.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// List your playlists.
    Playlists,
    /// Download tracks, a playlist, or an album (Hi-Res DASH is stitched).
    Download {
        #[command(subcommand)]
        target: DownloadTarget,
        /// Output directory (default: ~/Music/Tiders/<name>).
        #[arg(long, global = true, value_name = "DIR")]
        dest: Option<PathBuf>,
    },
    /// Run the background engine (IPC socket + MPRIS).
    Daemon {
        /// Unix socket path (default: $XDG_RUNTIME_DIR/tiders.sock).
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
    },
    /// Send a command to a running daemon.
    Ctl {
        /// Unix socket path (default: $XDG_RUNTIME_DIR/tiders.sock).
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
        #[command(subcommand)]
        action: CtlAction,
    },
    /// Launch the interactive terminal UI (default).
    Tui,
}

#[derive(Debug, Subcommand)]
pub enum DownloadTarget {
    /// Download every track in a playlist (UUID or tidal.com URL).
    Playlist { id: String },
    /// Download a single track by id.
    Track { track_id: u64 },
    /// Download every track on an album.
    Album { album_id: u64 },
}

#[derive(Debug, Subcommand)]
pub enum CtlAction {
    Status,
    Play,
    Pause,
    #[command(name = "play-pause")]
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek {
        seconds: f64,
    },
    Volume {
        volume: u8,
    },
    #[command(name = "play-track")]
    PlayTrack {
        track_id: u64,
    },
    #[command(name = "play-playlist")]
    PlayPlaylist {
        uuid: String,
    },
    #[command(name = "download-playlist")]
    DownloadPlaylist {
        uuid: String,
        #[arg(long)]
        dest: Option<PathBuf>,
    },
    Quit,
}

/// Parse arguments and run the selected command.
pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    let config = match &cli.config_dir {
        Some(dir) => Config::at(dir),
        None => Config::resolve().context("resolving config directory")?,
    };
    let settings = config.load_settings().unwrap_or_default();
    let quality = cli.quality.unwrap_or(settings.quality);

    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => {
            // No stderr logging here: it would corrupt the alternate screen.
            tui::run(config).await
        }
        command => {
            init_tracing();
            dispatch(command, config, settings, quality).await
        }
    }
}

async fn dispatch(
    command: Command,
    config: Config,
    settings: Settings,
    quality: Quality,
) -> Result<()> {
    match command {
        Command::Login => login(config, quality).await,
        Command::Logout => logout(config, quality).await,
        Command::Whoami => whoami(config, quality).await,
        Command::Search { query, limit } => {
            let service = require_service(config, quality).await?;
            let results = service.search(query.join(" "), limit).await?;
            output::print_search(&results);
            Ok(())
        }
        Command::Favorites { limit } => {
            let service = require_service(config, quality).await?;
            let tracks = service.favorite_tracks(limit, 0).await?;
            output::print_tracks("Favorite tracks", &tracks);
            println!();
            Ok(())
        }
        Command::Playlists => {
            let service = require_service(config, quality).await?;
            let playlists = service.playlists().await?;
            output::print_playlists(&playlists);
            println!();
            Ok(())
        }
        Command::Play { track_id } => play(config, settings, quality, track_id).await,
        Command::Download { target, dest } => download_cmd(config, quality, target, dest).await,
        Command::Daemon { socket } => daemon::run(config, settings, socket).await,
        Command::Ctl { socket, action } => ctl(socket, action).await,
        Command::Tui => unreachable!("handled in run()"),
    }
}

async fn require_service(config: Config, quality: Quality) -> Result<TidalService> {
    TidalService::restore(config, quality)
        .await?
        .ok_or_else(|| anyhow!("not signed in — run `tiders login` first"))
}

async fn login(config: Config, quality: Quality) -> Result<()> {
    let mut service = TidalService::new(config, quality);
    let device = service.begin_device_login().await?;

    println!("\n  Sign in to TIDAL\n");
    println!("  1. Open:  {}", device.url());
    println!("  2. Enter code:  {}\n", device.user_code);
    if open::that(device.url()).is_ok() {
        println!("  (opened the link in your browser)\n");
    }
    println!("  Waiting for authorization… (Ctrl-C to cancel)\n");

    // Print status updates as they arrive.
    let (tx, mut rx) = unbounded_channel::<OAuthStatus>();
    let printer = tokio::spawn(async move {
        let mut last = String::new();
        while let Some(status) = rx.recv().await {
            let line = match status {
                OAuthStatus::Waiting => "  … still waiting".to_string(),
                OAuthStatus::Success => "  ✓ authorized".to_string(),
                OAuthStatus::Error(e) => format!("  ✗ {e}"),
            };
            if line != last {
                println!("{line}");
                last = line;
            }
        }
    });

    service.complete_device_login(&device, Some(tx)).await?;
    let _ = printer.await;

    match service.username() {
        Some(user) => println!("\n  Signed in as {user}. Session saved.\n"),
        None => println!("\n  Signed in. Session saved.\n"),
    }
    Ok(())
}

async fn logout(config: Config, quality: Quality) -> Result<()> {
    match TidalService::restore(config.clone(), quality).await? {
        Some(mut service) => {
            service.logout().await?;
            println!("Signed out.");
        }
        None => println!("Not signed in."),
    }
    Ok(())
}

async fn whoami(config: Config, quality: Quality) -> Result<()> {
    match TidalService::restore(config, quality).await? {
        Some(service) => {
            println!("Signed in as: {}", service.username().unwrap_or_default());
            if let Some(country) = service.country() {
                println!("Country:      {country}");
            }
            println!("Quality:      {}", service.quality().label());
        }
        None => println!("Not signed in — run `tiders login`."),
    }
    Ok(())
}

async fn play(config: Config, settings: Settings, quality: Quality, track_id: u64) -> Result<()> {
    let mut service = require_service(config, quality).await?;
    let track = service.track(track_id).await?;
    let stream = service.stream_url_for(&track, quality).await?;

    let mut player = Player::with_backend(settings.backend, settings.volume)?;
    player.set_queue(vec![track.clone()], 0);
    player.play_current(&stream.url)?;

    println!(
        "\n  ▶ {}  [{}]  via {}",
        track.label(),
        quality.label(),
        player.backend_name()
    );
    if let Some(codecs) = &stream.quality.codecs {
        println!("     codec: {codecs}");
    }
    println!("     {}", stream.quality.label());

    if player.backend_name() == "null" {
        println!(
            "\n  Note: no audio backend available (install `mpv`). \
             Stream was resolved but not played.\n"
        );
        return Ok(());
    }

    println!("  Playing… press Ctrl-C to stop.\n");

    // Wait until the track ends or the user interrupts.
    loop {
        if player.poll_finished() {
            println!("  ✓ finished\n");
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = player.stop();
                println!("\n  ⏹ stopped\n");
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(400)) => {}
        }
        if player.status() == PlayerStatus::Stopped {
            break;
        }
    }
    Ok(())
}

async fn download_cmd(
    config: Config,
    quality: Quality,
    target: DownloadTarget,
    dest: Option<PathBuf>,
) -> Result<()> {
    let mut service = require_service(config, quality).await?;
    let (tracks, folder) = match target {
        DownloadTarget::Playlist { id } => {
            let uuid = download::parse_playlist_id(&id).to_string();
            let meta = service.playlist(&uuid).await.ok();
            let title = meta
                .as_ref()
                .map(|p| sanitize_dir(&p.title))
                .unwrap_or_else(|| uuid.clone());
            let tracks = service.playlist_tracks(&uuid).await?;
            if tracks.is_empty() {
                return Err(anyhow!("playlist is empty"));
            }
            println!(
                "Downloading playlist “{}” ({} tracks) at {}…",
                meta.as_ref().map(|p| p.title.as_str()).unwrap_or(&uuid),
                tracks.len(),
                quality.short_label()
            );
            (tracks, title)
        }
        DownloadTarget::Track { track_id } => {
            let track = service.track(track_id).await?;
            let folder = sanitize_dir(&track.title);
            println!(
                "Downloading {} at {}…",
                track.label(),
                quality.short_label()
            );
            (vec![track], folder)
        }
        DownloadTarget::Album { album_id } => {
            let tracks = service.album_tracks(album_id).await?;
            if tracks.is_empty() {
                return Err(anyhow!("album has no tracks"));
            }
            let folder = tracks
                .first()
                .and_then(|t| t.album.as_deref())
                .map(sanitize_dir)
                .unwrap_or_else(|| format!("album-{album_id}"));
            println!(
                "Downloading album “{folder}” ({} tracks) at {}…",
                tracks.len(),
                quality.short_label()
            );
            (tracks, folder)
        }
    };
    let dest = dest.unwrap_or_else(|| download::default_dest().join(folder));
    let report =
        download::download_tracks(&mut service, &tracks, &dest, quality, |p| match &p.status {
            DownloadStatus::Start => {
                println!("  [{}/{}] {}", p.index, p.total, p.track.label());
            }
            DownloadStatus::Done => {
                println!("       → {}", p.path.display());
            }
            DownloadStatus::Skip => {
                println!("       skip (exists) {}", p.path.display());
            }
            DownloadStatus::Failed(e) => {
                println!("       ✗ {e}");
            }
        })
        .await?;
    println!(
        "\n  {} saved, {} skipped, {} failed  → {}\n",
        report.ok(),
        report.items.iter().filter(|i| i.skipped).count(),
        report.failed(),
        report.dest.display()
    );
    if report.failed() > 0 {
        Err(anyhow!("{} track(s) failed", report.failed()))
    } else {
        Ok(())
    }
}

fn sanitize_dir(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_matches([' ', '.']).trim();
    if trimmed.is_empty() {
        "tiders".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

async fn ctl(socket: Option<PathBuf>, action: CtlAction) -> Result<()> {
    let path = socket.unwrap_or_else(ipc::socket_path);
    let cmd = match action {
        CtlAction::Status => EngineCommand::Status,
        CtlAction::Play => EngineCommand::Play,
        CtlAction::Pause => EngineCommand::Pause,
        CtlAction::PlayPause => EngineCommand::PlayPause,
        CtlAction::Stop => EngineCommand::Stop,
        CtlAction::Next => EngineCommand::Next,
        CtlAction::Previous => EngineCommand::Previous,
        CtlAction::Seek { seconds } => EngineCommand::Seek { seconds },
        CtlAction::Volume { volume } => EngineCommand::SetVolume { volume },
        CtlAction::PlayTrack { track_id } => EngineCommand::PlayTrack { id: track_id },
        CtlAction::PlayPlaylist { uuid } => EngineCommand::PlayPlaylist {
            uuid: download::parse_playlist_id(&uuid).to_string(),
        },
        CtlAction::DownloadPlaylist { uuid, dest } => EngineCommand::DownloadPlaylist {
            uuid: download::parse_playlist_id(&uuid).to_string(),
            dest,
        },
        CtlAction::Quit => EngineCommand::Quit,
    };
    let reply = ipc::send(&path, cmd)
        .await
        .with_context(|| format!("talking to daemon at {}", path.display()))?;
    if !reply.ok {
        return Err(anyhow!(
            "{}",
            reply.error.unwrap_or_else(|| "daemon error".into())
        ));
    }
    if let Some(state) = reply.state {
        println!(
            "{}  vol {}  queue {}  {}",
            match state.player.status {
                PlayerStatus::Playing => "playing",
                PlayerStatus::Paused => "paused",
                PlayerStatus::Stopped => "stopped",
            },
            state.player.volume,
            state.player.queue_len,
            state
                .player
                .now_playing
                .as_ref()
                .map(|t| t.label())
                .unwrap_or_default()
        );
        if state.duration_secs > 0.0 {
            println!(
                "  {:.0}/{:.0}s  {}",
                state.position_secs,
                state.duration_secs,
                state.player.quality.label()
            );
        }
    } else {
        println!("ok");
    }
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // Default to `warn`, but silence the expected `400 authorization_pending`
    // poll responses tidlers logs during the device-code login.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,tidlers::requests=error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
