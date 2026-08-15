//! An [`AudioBackend`] backed by a long-lived out-of-process `mpv`.
//!
//! `mpv` is spawned once (`--idle=yes`) and steered over its JSON IPC socket so
//! successive tracks are gapless, OS media keys keep a stable player identity,
//! and we can query decoder properties (sample rate, format, codec) live.
//!
//! On macOS we opt into `--input-media-keys` + `--macos-app-activation-policy`
//! so Control Center / Now Playing pick the process up. On Linux the TUI also
//! exports MPRIS; media keys here are a useful extra.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::format;
use crate::model::StreamQuality;

use super::backend::AudioBackend;

static IPC_COUNTER: AtomicU64 = AtomicU64::new(0);
static REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Playback backend that drives a persistent `mpv` subprocess.
pub struct MpvBackend {
    child: Option<Child>,
    ipc_path: std::path::PathBuf,
    volume: u8,
    replaygain: String,
    /// Set once we have observed EOF for the current file.
    reported_finished: bool,
    started_file: bool,
}

impl MpvBackend {
    /// Whether an `mpv` executable is discoverable on `PATH`.
    pub fn is_available() -> bool {
        which_mpv().is_some()
    }

    /// Create a new backend, failing if `mpv` cannot be found. Does not spawn
    /// until the first [`AudioBackend::play`].
    pub fn new(volume: u8) -> Result<Self> {
        Self::with_replaygain(volume, "album")
    }

    pub fn with_replaygain(volume: u8, replaygain: &str) -> Result<Self> {
        if !Self::is_available() {
            return Err(Error::Backend(
                "`mpv` not found on PATH — install it (e.g. `apt install mpv` or `brew install mpv`)".into(),
            ));
        }
        Ok(Self {
            child: None,
            ipc_path: unique_ipc_path(),
            volume: volume.min(100),
            replaygain: replaygain.to_string(),
            reported_finished: false,
            started_file: false,
        })
    }

    fn ensure_alive(&mut self) -> Result<()> {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return Ok(()),
                _ => {
                    self.child = None;
                }
            }
        }
        self.spawn_idle()
    }

    fn spawn_idle(&mut self) -> Result<()> {
        let _ = std::fs::remove_file(&self.ipc_path);
        self.ipc_path = unique_ipc_path();

        let mut command = Command::new("mpv");
        command
            .arg("--no-video")
            .arg("--no-terminal")
            .arg("--really-quiet")
            .arg("--idle=yes")
            .arg("--prefetch-playlist=yes")
            .arg("--gapless-audio=yes")
            .arg("--input-media-keys=yes")
            .arg("--force-window=no")
            .arg(format!("--volume={}", self.volume))
            .arg(format!("--replaygain={}", self.replaygain))
            .arg("--replaygain-clip=yes")
            .arg("--demuxer-lavf-o=protocol_whitelist=[file,crypto,data,https,tls,tcp,http]")
            .arg(format!("--input-ipc-server={}", self.ipc_path.display()));

        if let Some(ao) = std::env::var_os("TIDERS_MPV_AO") {
            command.arg(format!("--ao={}", ao.to_string_lossy()));
        }

        #[cfg(target_os = "macos")]
        {
            command.arg("--vo=null");
            command.arg("--macos-app-activation-policy=accessory");
        }

        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Backend(format!("failed to spawn mpv: {e}")))?;
        self.child = Some(child);

        // Wait briefly for the IPC socket to appear.
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            if self.ipc_path.exists() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    fn send_command(&self, command: &serde_json::Value) -> Result<()> {
        send_ipc(&self.ipc_path, command, None).map(|_| ())
    }

    fn get_property(&self, name: &str) -> Option<serde_json::Value> {
        let cmd = serde_json::json!(["get_property", name]);
        let reply = send_ipc(&self.ipc_path, &cmd, Some(Duration::from_millis(80))).ok()?;
        if reply.get("error").and_then(|e| e.as_str()) == Some("success") {
            reply.get("data").cloned()
        } else {
            None
        }
    }

    fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = self.send_command(&serde_json::json!(["quit"]));
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.ipc_path);
        self.started_file = false;
        self.reported_finished = false;
    }
}

impl AudioBackend for MpvBackend {
    fn play(&mut self, url: &str) -> Result<()> {
        self.ensure_alive()?;
        self.reported_finished = false;
        self.started_file = true;
        let _ = self.send_command(&serde_json::json!(["set_property", "volume", self.volume]));
        self.send_command(&serde_json::json!(["loadfile", url, "replace"]))
    }

    fn pause(&mut self) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!(["set_property", "pause", true]));
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!(["set_property", "pause", false]));
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!(["stop"]));
        }
        self.started_file = false;
        self.reported_finished = false;
        Ok(())
    }

    fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.volume = volume.min(100);
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!([
                "set_property",
                "volume",
                self.volume
            ]));
        }
        Ok(())
    }

    fn poll_finished(&mut self) -> bool {
        if self.reported_finished || !self.started_file {
            return false;
        }
        if let Some(child) = self.child.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                self.reported_finished = true;
                self.child = None;
                self.started_file = false;
                let _ = std::fs::remove_file(&self.ipc_path);
                return true;
            }
        }
        // idle-active becomes true when `--idle=yes` finishes a file.
        let idle = self
            .get_property("idle-active")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let eof = self
            .get_property("eof-reached")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if idle || eof {
            self.reported_finished = true;
            self.started_file = false;
            return true;
        }
        false
    }

    fn name(&self) -> &'static str {
        "mpv"
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!(["seek", seconds, "absolute"]));
        }
        Ok(())
    }

    fn position(&mut self) -> Option<f64> {
        self.get_property("time-pos")?.as_f64()
    }

    fn duration(&mut self) -> Option<f64> {
        self.get_property("duration")?.as_f64()
    }

    fn stream_quality(&mut self) -> StreamQuality {
        let samplerate = self
            .get_property("audio-params/samplerate")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        let channels = self
            .get_property("audio-params/channel-count")
            .and_then(|v| v.as_u64())
            .map(|n| n as u8);
        let format = self
            .get_property("audio-params/format")
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let codec = self
            .get_property("audio-codec-name")
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let bitrate = self
            .get_property("audio-bitrate")
            .and_then(|v| v.as_f64())
            .map(|n| n as u32);
        StreamQuality {
            audio_quality: None,
            mime_type: None,
            codecs: codec,
            sample_rate_hz: samplerate,
            bit_depth: format.as_deref().and_then(format::bit_depth_from_format),
            channels,
            bitrate_bps: bitrate,
        }
    }

    fn set_media_title(&mut self, title: &str) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!([
                "set_property",
                "force-media-title",
                title
            ]));
        }
        Ok(())
    }

    fn set_loop_file(&mut self, on: bool) -> Result<()> {
        if self.child.is_some() {
            let val = if on { "inf" } else { "no" };
            let _ = self.send_command(&serde_json::json!(["set_property", "loop-file", val]));
        }
        Ok(())
    }
}

impl Drop for MpvBackend {
    fn drop(&mut self) {
        self.kill_child();
    }
}

fn which_mpv() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("mpv");
        if candidate.is_file() {
            return Some(candidate);
        }
        let exe = dir.join("mpv.exe");
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

fn unique_ipc_path() -> std::path::PathBuf {
    let n = IPC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("tiders-mpv-{pid}-{n}.sock"))
}

#[cfg(unix)]
fn send_ipc(
    path: &std::path::Path,
    command: &serde_json::Value,
    wait: Option<Duration>,
) -> Result<serde_json::Value> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(path)
        .map_err(|e| Error::Backend(format!("mpv IPC connect failed: {e}")))?;
    if let Some(timeout) = wait {
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
    } else {
        let _ = stream.set_write_timeout(Some(Duration::from_millis(80)));
    }

    let req_id = REQ_COUNTER.fetch_add(1, Ordering::Relaxed);
    let payload = serde_json::json!({
        "command": command,
        "request_id": req_id,
    });
    let mut line = serde_json::to_vec(&payload)?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|e| Error::Backend(format!("mpv IPC write failed: {e}")))?;

    if wait.is_none() {
        return Ok(serde_json::Value::Null);
    }

    let mut reader = BufReader::new(stream);
    let deadline = Instant::now() + wait.unwrap();
    loop {
        if Instant::now() > deadline {
            return Err(Error::Backend("mpv IPC read timed out".into()));
        }
        let mut buf = String::new();
        match reader.read_line(&mut buf) {
            Ok(0) => return Err(Error::Backend("mpv IPC closed".into())),
            Ok(_) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(buf.trim()) {
                    if v.get("request_id").and_then(|id| id.as_u64()) == Some(req_id) {
                        return Ok(v);
                    }
                    // Event line (file-loaded, …) — keep reading.
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Err(Error::Backend("mpv IPC read timed out".into()));
            }
            Err(e) => return Err(Error::Backend(format!("mpv IPC read failed: {e}"))),
        }
    }
}

#[cfg(not(unix))]
fn send_ipc(
    _path: &std::path::Path,
    _command: &serde_json::Value,
    _wait: Option<Duration>,
) -> Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
