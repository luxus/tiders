//! Offline download of tracks / playlists.
//!
//! Direct FLAC/AAC URLs are saved as-is. Hi-Res DASH is stitched into an
//! fMP4 then remuxed to FLAC with `ffmpeg -c copy` when ffmpeg is on PATH
//! (same approach as yadal / streamrip). Without ffmpeg the fMP4 is kept.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::config::Quality;
use crate::dash::{self, DashAssembly};
use crate::error::{Error, Result};
use crate::model::TrackView;
use crate::session::TidalService;

/// One item in a download batch.
#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub track: TrackView,
    pub path: PathBuf,
    pub skipped: bool,
    pub error: Option<String>,
}

/// Summary of a playlist / album / track download.
#[derive(Debug, Clone, Default)]
pub struct DownloadReport {
    pub dest: PathBuf,
    pub items: Vec<DownloadItem>,
}

impl DownloadReport {
    pub fn ok(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.error.is_none() && !i.skipped)
            .count()
    }

    pub fn failed(&self) -> usize {
        self.items.iter().filter(|i| i.error.is_some()).count()
    }
}

/// Progress callback payload (1-based index).
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub index: usize,
    pub total: usize,
    pub track: TrackView,
    pub path: PathBuf,
    pub status: DownloadStatus,
}

#[derive(Debug, Clone)]
pub enum DownloadStatus {
    Start,
    Done,
    Skip,
    Failed(String),
}

/// Default folder: XDG music dir / `Tiders`, else `~/Music/Tiders`.
pub fn default_dest() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Tiders")
}

/// Download every track in `tracks` into `dest` (created if missing).
pub async fn download_tracks(
    service: &mut TidalService,
    tracks: &[TrackView],
    dest: &Path,
    quality: Quality,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<DownloadReport> {
    std::fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;
    let total = tracks.len();
    let width = total.max(1).to_string().len().max(2);
    let mut report = DownloadReport {
        dest: dest.to_path_buf(),
        items: Vec::with_capacity(total),
    };
    for (i, track) in tracks.iter().enumerate() {
        let filename = track_filename(i + 1, width, track, quality);
        let path = dest.join(&filename);
        on_progress(DownloadProgress {
            index: i + 1,
            total,
            track: track.clone(),
            path: path.clone(),
            status: DownloadStatus::Start,
        });
        if path.exists() {
            on_progress(DownloadProgress {
                index: i + 1,
                total,
                track: track.clone(),
                path: path.clone(),
                status: DownloadStatus::Skip,
            });
            report.items.push(DownloadItem {
                track: track.clone(),
                path,
                skipped: true,
                error: None,
            });
            continue;
        }
        match download_one(service, track, &path, quality).await {
            Ok(()) => {
                on_progress(DownloadProgress {
                    index: i + 1,
                    total,
                    track: track.clone(),
                    path: path.clone(),
                    status: DownloadStatus::Done,
                });
                report.items.push(DownloadItem {
                    track: track.clone(),
                    path,
                    skipped: false,
                    error: None,
                });
            }
            Err(e) => {
                let msg = e.to_string();
                on_progress(DownloadProgress {
                    index: i + 1,
                    total,
                    track: track.clone(),
                    path: path.clone(),
                    status: DownloadStatus::Failed(msg.clone()),
                });
                let _ = std::fs::remove_file(&path);
                report.items.push(DownloadItem {
                    track: track.clone(),
                    path,
                    skipped: false,
                    error: Some(msg),
                });
            }
        }
    }
    Ok(report)
}

async fn download_one(
    service: &mut TidalService,
    track: &TrackView,
    dest: &Path,
    quality: Quality,
) -> Result<()> {
    let playback = service.playback_info(track.id, quality).await?;
    match playback.manifest_parsed.as_ref() {
        Some(tidlers::client::models::track::playback::ParsedTrackManifest::Dash(dash)) => {
            let assembly = dash::assemble(dash, track.duration_secs)?;
            stitch_and_remux(&assembly, dest).await
        }
        _ => {
            let url = playback.get_primary_url().ok_or(Error::NoStream)?;
            download_url(&url, dest).await
        }
    }
}

async fn stitch_and_remux(assembly: &DashAssembly, dest: &Path) -> Result<()> {
    let tmp = dest.with_extension("dash.mp4");
    dash::stitch_to_file(assembly, &tmp).await?;
    if dest.extension().and_then(|e| e.to_str()) == Some("flac") && ffmpeg_available() {
        remux_flac(&tmp, dest)?;
        let _ = std::fs::remove_file(&tmp);
        return Ok(());
    }
    std::fs::rename(&tmp, dest).map_err(|e| Error::io(dest, e))
}

async fn download_url(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("tiders/0.1")
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| Error::other(format!("http client: {e}")))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::other(format!("download: {e}")))?
        .error_for_status()
        .map_err(|e| Error::other(format!("download status: {e}")))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let part = sibling_part(dest);
    let result = async {
        let mut file = std::fs::File::create(&part).map_err(|e| Error::io(&part, e))?;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::other(format!("download body: {e}")))?
        {
            file.write_all(&chunk).map_err(|e| Error::io(&part, e))?;
        }
        file.flush().map_err(|e| Error::io(&part, e))?;
        drop(file);
        std::fs::rename(&part, dest).map_err(|e| Error::io(dest, e))
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

fn sibling_part(dest: &Path) -> PathBuf {
    match dest.file_name() {
        Some(name) => {
            let mut name = name.to_os_string();
            name.push(".part");
            dest.with_file_name(name)
        }
        None => dest.join("download.part"),
    }
}

fn remux_flac(src: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-c:a", "copy"])
        .arg(dest)
        .status()
        .map_err(|e| Error::other(format!("ffmpeg: {e}")))?;
    if !status.success() {
        return Err(Error::other(format!(
            "ffmpeg remux failed with status {status}"
        )));
    }
    Ok(())
}

fn ffmpeg_available() -> bool {
    static YES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *YES.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// `01 - Artist - Title.flac` (or `.m4a` / `.mp4` depending on quality).
pub fn track_filename(index: usize, width: usize, track: &TrackView, quality: Quality) -> String {
    let ext = match quality {
        Quality::Low | Quality::High => "m4a",
        Quality::Lossless | Quality::HiRes => {
            if ffmpeg_available() {
                "flac"
            } else {
                "mp4"
            }
        }
    };
    let artist = sanitize(&track.artist);
    let title = sanitize(&track.title);
    format!("{index:0width$} - {artist} - {title}.{ext}")
}

fn sanitize(s: &str) -> String {
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
        "track".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// Pull a playlist UUID out of a raw id or a tidal.com URL.
pub fn parse_playlist_id(raw: &str) -> &str {
    let s = raw.trim();
    if let Some(rest) = s
        .split("playlist/")
        .nth(1)
        .or_else(|| s.split("playlists/").nth(1))
    {
        return rest
            .split(['?', '/', '&', '#'])
            .next()
            .unwrap_or(rest)
            .trim();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> TrackView {
        TrackView {
            id: 1,
            title: "Digital / Love".into(),
            artist: "Daft Punk".into(),
            album: Some("Discovery".into()),
            duration_secs: 301,
            ..TrackView::default()
        }
    }

    #[test]
    fn filename_strips_slashes() {
        let name = track_filename(3, 2, &track(), Quality::Low);
        assert!(name.starts_with("03 - Daft Punk - Digital _ Love.m4a"));
        assert!(!name.contains('/'));
    }

    #[test]
    fn playlist_id_from_url() {
        assert_eq!(
            parse_playlist_id("aa692128-2954-4fe1-b5a1-4ede1add485d"),
            "aa692128-2954-4fe1-b5a1-4ede1add485d"
        );
        assert_eq!(
            parse_playlist_id(
                "https://listen.tidal.com/playlist/aa692128-2954-4fe1-b5a1-4ede1add485d?play=true"
            ),
            "aa692128-2954-4fe1-b5a1-4ede1add485d"
        );
    }

    #[test]
    fn part_file_keeps_original_filename() {
        let dest = PathBuf::from("/tmp/01 - Artist - Title.flac");
        assert_eq!(
            sibling_part(&dest),
            PathBuf::from("/tmp/01 - Artist - Title.flac.part")
        );
    }
}
