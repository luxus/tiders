//! An [`AudioBackend`] backed by an out-of-process `mpv`.
//!
//! `mpv` is a small, ubiquitous, cross-platform media player available on both
//! macOS (`brew install mpv`) and Linux. It transparently handles every stream
//! shape TIDAL hands back — direct FLAC URLs, DASH, and HLS — which is why the
//! original Maré Player leaned on GStreamer for the same job. Rather than link a
//! heavy native library, we spawn `mpv` and steer it over its JSON IPC socket.
//!
//! Only one track plays at a time; [`play`](AudioBackend::play) replaces any
//! existing process.

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Error, Result};

use super::backend::AudioBackend;

static IPC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Playback backend that drives an `mpv` subprocess.
pub struct MpvBackend {
    child: Option<Child>,
    ipc_path: std::path::PathBuf,
    volume: u8,
    /// Set once we have observed the current child exit (so `poll_finished`
    /// only reports a finished track a single time).
    reported_finished: bool,
}

impl MpvBackend {
    /// Whether an `mpv` executable is discoverable on `PATH`.
    pub fn is_available() -> bool {
        which_mpv().is_some()
    }

    /// Create a new backend, failing if `mpv` cannot be found.
    pub fn new(volume: u8) -> Result<Self> {
        if !Self::is_available() {
            return Err(Error::Backend(
                "`mpv` not found on PATH — install it (e.g. `apt install mpv` or `brew install mpv`)".into(),
            ));
        }
        let ipc_path = unique_ipc_path();
        Ok(Self {
            child: None,
            ipc_path,
            volume: volume.min(100),
            reported_finished: false,
        })
    }

    /// Send a single JSON-IPC command object to the running `mpv`.
    fn send_command(&self, command: &serde_json::Value) -> Result<()> {
        send_ipc(&self.ipc_path, command)
    }

    fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.ipc_path);
    }
}

impl AudioBackend for MpvBackend {
    fn play(&mut self, url: &str) -> Result<()> {
        self.kill_child();
        // Fresh socket path per track avoids racing a just-removed socket.
        self.ipc_path = unique_ipc_path();
        self.reported_finished = false;

        let mut command = Command::new("mpv");
        command
            .arg("--no-video")
            .arg("--no-terminal")
            .arg("--really-quiet")
            .arg("--idle=no")
            .arg(format!("--volume={}", self.volume))
            .arg(format!("--input-ipc-server={}", self.ipc_path.display()));

        // Optional audio-output override. Handy on headless machines/CI where
        // there is no sound device: `TIDERS_MPV_AO=null` decodes the stream in
        // real time without opening an output. Unset on a normal desktop.
        if let Some(ao) = std::env::var_os("TIDERS_MPV_AO") {
            command.arg(format!("--ao={}", ao.to_string_lossy()));
        }

        let child = command
            .arg("--")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Backend(format!("failed to spawn mpv: {e}")))?;

        self.child = Some(child);
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!({
                "command": ["set_property", "pause", true]
            }));
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!({
                "command": ["set_property", "pause", false]
            }));
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.kill_child();
        Ok(())
    }

    fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.volume = volume.min(100);
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!({
                "command": ["set_property", "volume", self.volume]
            }));
        }
        Ok(())
    }

    fn poll_finished(&mut self) -> bool {
        if self.reported_finished {
            return false;
        }
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Process exited: the track ran to completion (or mpv died).
                self.reported_finished = true;
                self.child = None;
                let _ = std::fs::remove_file(&self.ipc_path);
                true
            }
            _ => false,
        }
    }

    fn name(&self) -> &'static str {
        "mpv"
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
        // Windows fallback (not a primary target, but harmless).
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
fn send_ipc(path: &std::path::Path, command: &serde_json::Value) -> Result<()> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(path)
        .map_err(|e| Error::Backend(format!("mpv IPC connect failed: {e}")))?;
    let mut line = serde_json::to_vec(command)?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|e| Error::Backend(format!("mpv IPC write failed: {e}")))?;
    Ok(())
}

#[cfg(not(unix))]
fn send_ipc(_path: &std::path::Path, _command: &serde_json::Value) -> Result<()> {
    // On non-unix targets we skip live control; playback still works, it just
    // can't be paused/volume-adjusted mid-track. macOS + Linux are both unix.
    Ok(())
}
