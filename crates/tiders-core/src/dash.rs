//! Hi-Res DASH assembly: expand a TIDAL MPD into segment URLs, stitch them
//! into a single fMP4, or rewrite a static MPD that mpv can play.
//!
//! TIDAL's `HI_RES` / `HI_RES_LOSSLESS` tier is MPEG-DASH (FLAC in fragmented
//! MP4), not a single FLAC URL. [`TidalService::stream_url`](crate::session::TidalService::stream_url)
//! used to hand mpv `DashManifest::urls[0]` — usually the **init** segment —
//! so Hi-Res playback never actually assembled the track. This module is the
//! shared stitcher used by playback (write an MPD) and downloads (concat +
//! remux).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tidlers::client::models::track::playback::DashManifest;

use crate::error::{Error, Result};
use crate::media;

/// A fully resolved DASH presentation: init URL + numbered media segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashAssembly {
    pub mime_type: String,
    pub codecs: String,
    pub bitrate: Option<u32>,
    pub init_url: String,
    pub media_urls: Vec<String>,
}

impl DashAssembly {
    /// Init segment followed by every media segment, in playback order.
    pub fn urls(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.init_url.as_str()).chain(self.media_urls.iter().map(String::as_str))
    }
}

/// Expand a parsed TIDAL DASH manifest into concrete segment URLs.
///
/// `track_secs` sizes the `$Number$` run when the MPD only has a per-segment
/// duration (typical for TIDAL). Pass `0` to keep a conservative 1-hour cap
/// and let HTTP 404 stop the download loop.
pub fn assemble(dash: &DashManifest, track_secs: u64) -> Result<DashAssembly> {
    let init_rel = dash
        .get_init_url()
        .ok_or_else(|| Error::other("DASH manifest has no initialization segment"))?;
    let init_url = resolve_url(dash, init_rel);
    let start = dash.start_number.unwrap_or(1);
    let count = segment_count(dash, track_secs);
    let mut media_urls = Vec::with_capacity(count as usize);
    for n in start..start.saturating_add(count) {
        let Some(raw) = expand_segment_url(dash, n) else {
            break;
        };
        media_urls.push(resolve_url(dash, &raw));
    }
    if media_urls.is_empty() {
        return Err(Error::other("DASH manifest produced no media segments"));
    }
    Ok(DashAssembly {
        mime_type: dash.mime_type.clone(),
        codecs: dash.codecs.clone(),
        bitrate: dash.bitrate,
        init_url,
        media_urls,
    })
}

/// How many `$Number$` media segments the track needs.
pub fn segment_count(dash: &DashManifest, track_secs: u64) -> u32 {
    const HOUR_CAP: u32 = 3600;
    if let (Some(timescale), Some(seg_dur)) = (dash.timescale, dash.duration) {
        if timescale > 0 && seg_dur > 0 {
            let secs = track_secs.max(1);
            let ticks = secs.saturating_mul(timescale as u64);
            let n = ticks.div_ceil(seg_dur as u64);
            return (n as u32).clamp(1, HOUR_CAP);
        }
    }
    track_secs.max(1).min(HOUR_CAP as u64) as u32
}

/// Write a static MPD with **absolute** segment URLs so mpv's dash demuxer
/// can fetch them. Returns the path to the `.mpd`.
pub fn write_playback_mpd(
    cache_dir: &Path,
    track_id: u64,
    dash: &DashManifest,
    track_secs: u64,
) -> Result<PathBuf> {
    let assembly = assemble(dash, track_secs)?;
    let dash_dir = cache_dir.join("dash");
    std::fs::create_dir_all(&dash_dir).map_err(|e| Error::io(&dash_dir, e))?;
    let path = dash_dir.join(format!("{track_id}.mpd"));
    let xml = render_mpd(&assembly, dash, track_secs);
    std::fs::write(&path, xml).map_err(|e| Error::io(&path, e))?;
    Ok(path)
}

/// `file://` URL for a path, suitable for mpv `loadfile`.
pub fn file_url(path: &Path) -> Result<String> {
    media::file_url(path).ok_or_else(|| Error::other("DASH cache path is not valid UTF-8"))
}

/// Concatenate init + media segments into `dest` (typically a `.mp4` / `.m4a`
/// next to the final FLAC).
///
/// Segments are streamed to a sibling `.part` file and renamed on success so a
/// failed stitch does not leave a truncated dest. Stops after three consecutive
/// missing segments (TIDAL returns 404 past the last chunk). Playback prefers
/// [`write_playback_mpd`].
pub async fn stitch_to_file(assembly: &DashAssembly, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let part = sibling_part(dest);
    match stitch_into(assembly, &part).await {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            Error::io(dest, e)
        }),
        Err(err) => {
            let _ = std::fs::remove_file(&part);
            Err(err)
        }
    }
}

async fn stitch_into(assembly: &DashAssembly, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("tiders/0.1")
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| Error::other(format!("http client: {e}")))?;

    let mut file = std::fs::File::create(dest).map_err(|e| Error::io(dest, e))?;
    copy_segment(&client, &assembly.init_url, &mut file).await?;

    let mut consecutive_misses = 0u8;
    let mut media_n = 0usize;
    for url in &assembly.media_urls {
        match copy_segment(&client, url, &mut file).await {
            Ok(_) => {
                consecutive_misses = 0;
                media_n += 1;
            }
            Err(e) if is_missing(&e) => {
                consecutive_misses += 1;
                if consecutive_misses >= 3 {
                    break;
                }
            }
            Err(e) => return Err(e),
        }
    }
    file.flush().map_err(|e| Error::io(dest, e))?;
    drop(file);
    if media_n == 0 {
        return Err(Error::other("DASH stitch downloaded no media segments"));
    }
    Ok(())
}

async fn copy_segment(client: &reqwest::Client, url: &str, dest: &mut impl Write) -> Result<u64> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::other(format!("DASH segment: {e}")))?;
    let status = response.status();
    if status.as_u16() == 404 {
        return Err(Error::other("DASH segment 404"));
    }
    if !status.is_success() {
        return Err(Error::other(format!("DASH segment HTTP {status}")));
    }
    let mut n = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::other(format!("DASH segment body: {e}")))?
    {
        dest.write_all(&chunk)
            .map_err(|e| Error::other(format!("DASH segment write: {e}")))?;
        n += chunk.len() as u64;
    }
    if n == 0 {
        return Err(Error::other("DASH segment empty"));
    }
    Ok(n)
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

fn is_missing(err: &Error) -> bool {
    err.to_string().contains("404")
}

/// Join a possibly-relative DASH URL against the manifest BaseURL.
pub fn resolve_url(dash: &DashManifest, url: &str) -> String {
    if looks_absolute(url) {
        return url.to_string();
    }
    let base = dash
        .urls
        .iter()
        .find(|u| looks_absolute(u))
        .map(String::as_str)
        .unwrap_or("");
    if base.is_empty() {
        return url.to_string();
    }
    if base.ends_with('/') {
        format!("{base}{url}")
    } else {
        format!("{base}/{url}")
    }
}

fn looks_absolute(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("data:")
}

/// Expand `$Number$` / `$Number%0Nd$` the way a DASH player would.
pub fn expand_segment_url(dash: &DashManifest, number: u32) -> Option<String> {
    let template = dash.get_media_template()?;
    Some(expand_number_template(template, number))
}

pub fn expand_number_template(template: &str, number: u32) -> String {
    let mut out = template.replace("$Number$", &number.to_string());
    // `$Number%05d$` (width + zero-pad).
    while let Some(start) = out.find("$Number%") {
        let rest = &out[start + 8..];
        let Some(end_rel) = rest.find('$') else {
            break;
        };
        let spec = &rest[..end_rel];
        let width = spec
            .trim_start_matches('0')
            .trim_end_matches('d')
            .parse::<usize>()
            .ok()
            .or_else(|| spec.trim_end_matches('d').parse().ok())
            .unwrap_or(0);
        let padded = if width == 0 {
            number.to_string()
        } else {
            format!("{number:0width$}")
        };
        let token_end = start + 8 + end_rel + 1;
        out.replace_range(start..token_end, &padded);
    }
    out
}

fn render_mpd(assembly: &DashAssembly, dash: &DashManifest, track_secs: u64) -> String {
    let mime = xml_attr(if assembly.mime_type.is_empty() {
        "audio/mp4"
    } else {
        &assembly.mime_type
    });
    let codecs = xml_attr(&assembly.codecs);
    let bandwidth = assembly.bitrate.unwrap_or(1_411_000);
    let timescale = dash.timescale.unwrap_or(1);
    let seg_dur = dash.duration.unwrap_or(timescale.max(1));
    let start = dash.start_number.unwrap_or(1);
    let duration_attr = if track_secs > 0 {
        format!(" mediaPresentationDuration=\"PT{track_secs}S\"")
    } else {
        String::new()
    };
    let init = xml_attr(&assembly.init_url);
    // Keep the numbered template (absolute) so mpv fetches lazily instead of
    // requiring every segment URL inline.
    let media = match dash.get_media_template() {
        Some(t) => xml_attr(&resolve_url(dash, t)),
        None => xml_attr(
            assembly
                .media_urls
                .first()
                .map(String::as_str)
                .unwrap_or(""),
        ),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" profiles="urn:mpeg:dash:profile:isoff-live:2011" type="static" minBufferTime="PT2S"{duration_attr}>
  <Period>
    <AdaptationSet mimeType="{mime}" codecs="{codecs}" segmentAlignment="true">
      <Representation id="1" bandwidth="{bandwidth}" codecs="{codecs}">
        <SegmentTemplate initialization="{init}" media="{media}" startNumber="{start}" timescale="{timescale}" duration="{seg_dur}"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#
    )
}

fn xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dash() -> DashManifest {
        DashManifest {
            mime_type: "audio/mp4".into(),
            codecs: "flac".into(),
            urls: vec!["https://audio.example.com/".into()],
            bitrate: Some(9_216_000),
            initialization_url: Some("init.mp4".into()),
            media_url_template: Some("chunk-$Number$.m4s".into()),
            timescale: Some(48_000),
            duration: Some(96_000),
            start_number: Some(1),
        }
    }

    #[test]
    fn resolve_joins_baseurl() {
        let d = dash();
        assert_eq!(
            resolve_url(&d, "init.mp4"),
            "https://audio.example.com/init.mp4"
        );
        assert_eq!(
            resolve_url(&d, "https://cdn.example/x.m4s"),
            "https://cdn.example/x.m4s"
        );
    }

    #[test]
    fn number_template_pads() {
        assert_eq!(expand_number_template("seg-$Number$.m4s", 7), "seg-7.m4s");
        assert_eq!(
            expand_number_template("seg-$Number%05d$.m4s", 7),
            "seg-00007.m4s"
        );
    }

    #[test]
    fn assemble_uses_duration_and_timescale() {
        // 10s track, 96_000 ticks / 48_000 = 2s per segment → 5 segments.
        let a = assemble(&dash(), 10).unwrap();
        assert_eq!(a.init_url, "https://audio.example.com/init.mp4");
        assert_eq!(a.media_urls.len(), 5);
        assert_eq!(a.media_urls[0], "https://audio.example.com/chunk-1.m4s");
        assert_eq!(a.media_urls[4], "https://audio.example.com/chunk-5.m4s");
    }

    #[test]
    fn mpd_escapes_query_ampersands() {
        let mut d = dash();
        d.initialization_url = Some("https://cdn.example/init.mp4?token=a&b=1".into());
        d.media_url_template = Some("https://cdn.example/chunk-$Number$.m4s?token=a&b=1".into());
        let xml = render_mpd(&assemble(&d, 4).unwrap(), &d, 4);
        assert!(xml.contains("&amp;"), "ampersands must be escaped: {xml}");
        assert!(!xml.contains("token=a&b=1"), "raw ampersand leaked: {xml}");
        assert!(xml.contains("mediaPresentationDuration=\"PT4S\""));
        assert!(xml.contains("type=\"static\""));
    }

    #[test]
    fn write_playback_mpd_creates_file() {
        let dir = std::env::temp_dir().join(format!(
            "tiders-dash-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let path = write_playback_mpd(&dir, 42, &dash(), 8).unwrap();
        assert!(path.ends_with("42.mpd"));
        let xml = std::fs::read_to_string(&path).unwrap();
        assert!(xml.contains("chunk-$Number$.m4s"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stitch_streams_to_file_and_stops_on_404() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 1024];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/");
                    let (status, body): (&str, &[u8]) = match path {
                        "/init.mp4" => ("200 OK", b"INIT"),
                        "/chunk-1.m4s" => ("200 OK", b"A"),
                        "/chunk-2.m4s" => ("200 OK", b"B"),
                        _ => ("404 Not Found", b""),
                    };
                    let header = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });

        let assembly = DashAssembly {
            mime_type: "audio/mp4".into(),
            codecs: "flac".into(),
            bitrate: None,
            init_url: format!("http://{addr}/init.mp4"),
            media_urls: vec![
                format!("http://{addr}/chunk-1.m4s"),
                format!("http://{addr}/chunk-2.m4s"),
                format!("http://{addr}/chunk-3.m4s"),
                format!("http://{addr}/chunk-4.m4s"),
                format!("http://{addr}/chunk-5.m4s"),
            ],
        };
        let dir = std::env::temp_dir().join(format!(
            "tiders-stitch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let dest = dir.join("out.mp4");
        stitch_to_file(&assembly, &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"INITAB");
        assert!(!sibling_part(&dest).exists(), "part file must be renamed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
