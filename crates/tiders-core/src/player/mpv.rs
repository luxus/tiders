//! An [`AudioBackend`] backed by a long-lived out-of-process `mpv`.
//!
//! `mpv` is spawned once (`--idle=yes`) and steered over its JSON IPC socket so
//! successive tracks are gapless, OS media keys keep a stable player identity,
//! and we can query decoder properties (sample rate, format, codec) live.
//!
//! A background observer thread holds a **persistent** IPC connection and
//! `observe_property`s — the UI thread never blocks on sockets. A second idle
//! `mpv` with `--ao=pcm` taps decoded samples into a FIFO for the rustfft
//! analyser without touching the speakers or media keys.
//!
//! On macOS we **disable** `--input-media-keys` so mpv does not steal Control
//! Center / Now Playing from Tiders (souvlaki). A cocoa activation policy on
//! mpv would register a second Now Playing identity named "mpv".

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::format;
use crate::model::StreamQuality;

use super::backend::AudioBackend;

static IPC_COUNTER: AtomicU64 = AtomicU64::new(0);
static REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Live decoder / clock state filled by the IPC observer (lock-free reads).
#[derive(Default)]
struct Live {
    time_pos: AtomicU64,
    duration: AtomicU64,
    eof: AtomicBool,
    idle: AtomicBool,
    paused: AtomicBool,
    /// Unix millis when `time_pos` was last stamped (for interpolation).
    pos_millis: AtomicU64,
    samplerate: AtomicU32,
    channels: AtomicU32,
    bit_depth: AtomicU32,
    bitrate: AtomicU32,
    codec: Mutex<Option<String>>,
    format: Mutex<Option<String>>,
    /// Decoded PCM from the visualiser tap (mono f32, ~44.1 kHz).
    pcm: Mutex<Vec<f32>>,
}

impl Live {
    fn stamp_pos(&self, seconds: f64) {
        self.time_pos.store(seconds.to_bits(), Ordering::Relaxed);
        self.pos_millis.store(unix_millis(), Ordering::Relaxed);
    }

    fn time_pos(&self) -> Option<f64> {
        let raw = f64::from_bits(self.time_pos.load(Ordering::Relaxed));
        if self.paused.load(Ordering::Relaxed) {
            return Some(raw);
        }
        let stamped = self.pos_millis.load(Ordering::Relaxed);
        if stamped == 0 {
            return Some(raw);
        }
        let extra = unix_millis().saturating_sub(stamped) as f64 / 1000.0;
        Some(raw + extra.min(2.0))
    }

    fn duration(&self) -> Option<f64> {
        let v = f64::from_bits(self.duration.load(Ordering::Relaxed));
        (v > 0.0).then_some(v)
    }
}

/// Playback backend that drives a persistent `mpv` subprocess.
pub struct MpvBackend {
    child: Option<Child>,
    vis: Option<Child>,
    ipc_path: PathBuf,
    vis_ipc: PathBuf,
    fifo_path: PathBuf,
    volume: u8,
    replaygain: String,
    reported_finished: bool,
    started_file: bool,
    live: Arc<Live>,
    /// Bumped on every respawn so observer / PCM threads exit.
    gen: Arc<AtomicU64>,
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
            vis: None,
            ipc_path: unique_path("mpv", "sock"),
            vis_ipc: unique_path("vis", "sock"),
            fifo_path: unique_path("pcm", "fifo"),
            volume: volume.min(100),
            replaygain: replaygain.to_string(),
            reported_finished: false,
            started_file: false,
            live: Arc::new(Live::default()),
            gen: Arc::new(AtomicU64::new(0)),
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
        self.stop_helpers();
        let _ = std::fs::remove_file(&self.ipc_path);
        self.ipc_path = unique_path("mpv", "sock");

        let mut command = Command::new("mpv");
        command
            .arg("--no-video")
            .arg("--no-terminal")
            .arg("--really-quiet")
            .arg("--idle=yes")
            .arg("--prefetch-playlist=yes")
            .arg("--gapless-audio=yes")
            // Tiders owns Now Playing / MPRIS (souvlaki). If mpv also registers
            // media keys it shows up as "mpv" with a combined title, no artist,
            // no artwork, and next/prev bound to an empty playlist.
            .arg("--input-media-keys=no")
            .arg("--force-window=no")
            .arg("--audio-display=no")
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
            // Headless audio only. Do **not** set macos-app-activation-policy —
            // that creates an NSApplication named "mpv" and hijacks Now Playing.
            command.arg("--vo=null");
        }

        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Backend(format!("failed to spawn mpv: {e}")))?;
        self.child = Some(child);

        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            if self.ipc_path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        spawn_observer(
            self.ipc_path.clone(),
            Arc::clone(&self.live),
            Arc::clone(&self.gen),
            gen,
        );
        self.spawn_vis(gen);
        Ok(())
    }

    fn spawn_vis(&mut self, gen: u64) {
        // A second mpv decoding the same stream into a FIFO is expensive and
        // was freezing the analyser. Opt in with TIDERS_PCM_VIS=1; otherwise
        // the rustfft visualiser uses its synth fallback.
        if std::env::var_os("TIDERS_PCM_VIS").is_none() {
            let _ = gen;
            return;
        }
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.vis_ipc);
            let _ = std::fs::remove_file(&self.fifo_path);
            self.vis_ipc = unique_path("vis", "sock");
            self.fifo_path = unique_path("pcm", "fifo");
            if mkfifo(&self.fifo_path).is_err() {
                return;
            }

            let mut command = Command::new("mpv");
            command
                .arg("--no-video")
                .arg("--no-terminal")
                .arg("--really-quiet")
                .arg("--idle=yes")
                .arg("--force-window=no")
                .arg("--input-media-keys=no")
                .arg("--untimed=no")
                .arg("--audio-display=no")
                .arg("--audio-format=s16")
                .arg("--audio-samplerate=44100")
                .arg("--audio-channels=mono")
                .arg("--ao=pcm")
                .arg("--ao-pcm-waveheader=no")
                .arg(format!("--ao-pcm-file={}", self.fifo_path.display()))
                .arg(format!("--input-ipc-server={}", self.vis_ipc.display()));

            match command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => self.vis = Some(child),
                Err(_) => {
                    let _ = std::fs::remove_file(&self.fifo_path);
                    return;
                }
            }

            spawn_pcm_reader(
                self.fifo_path.clone(),
                Arc::clone(&self.live),
                Arc::clone(&self.gen),
                gen,
            );
        }
        #[cfg(not(unix))]
        {
            let _ = gen;
        }
    }

    fn vis_cmd(&self, command: &serde_json::Value) {
        if self.vis.is_some() {
            let _ = send_ipc(&self.vis_ipc, command, None);
        }
    }

    fn send_command(&self, command: &serde_json::Value) -> Result<()> {
        send_ipc(&self.ipc_path, command, None).map(|_| ())
    }

    fn stop_helpers(&mut self) {
        if let Some(mut vis) = self.vis.take() {
            let _ = send_ipc(&self.vis_ipc, &serde_json::json!(["quit"]), None);
            let _ = vis.kill();
            let _ = vis.wait();
        }
        // Unblock a reader stuck on `open(fifo)` by briefly opening it for write.
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.fifo_path);
        let _ = std::fs::remove_file(&self.vis_ipc);
        let _ = std::fs::remove_file(&self.fifo_path);
        self.gen.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut pcm) = self.live.pcm.lock() {
            pcm.clear();
        }
    }

    fn kill_child(&mut self) {
        self.stop_helpers();
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
        self.live.eof.store(false, Ordering::Relaxed);
        self.live.idle.store(false, Ordering::Relaxed);
        self.live.paused.store(false, Ordering::Relaxed);
        self.live.stamp_pos(0.0);
        if let Ok(mut pcm) = self.live.pcm.lock() {
            pcm.clear();
        }
        let _ = self.send_command(&serde_json::json!(["set_property", "volume", self.volume]));
        self.send_command(&serde_json::json!(["loadfile", url, "replace"]))?;
        self.vis_cmd(&serde_json::json!(["set_property", "pause", false]));
        self.vis_cmd(&serde_json::json!(["loadfile", url, "replace"]));
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        if self.child.is_some() {
            let pos = self.live.time_pos().unwrap_or(0.0);
            self.live.paused.store(true, Ordering::Relaxed);
            self.live.time_pos.store(pos.to_bits(), Ordering::Relaxed);
            let _ = self.send_command(&serde_json::json!(["set_property", "pause", true]));
            self.vis_cmd(&serde_json::json!(["set_property", "pause", true]));
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.child.is_some() {
            self.live.paused.store(false, Ordering::Relaxed);
            self.live.pos_millis.store(unix_millis(), Ordering::Relaxed);
            let _ = self.send_command(&serde_json::json!(["set_property", "pause", false]));
            self.vis_cmd(&serde_json::json!(["set_property", "pause", false]));
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!(["stop"]));
            self.vis_cmd(&serde_json::json!(["stop"]));
        }
        self.started_file = false;
        self.reported_finished = false;
        Ok(())
    }

    fn set_volume(&mut self, volume: u8) -> Result<()> {
        self.volume = volume.min(100);
        if self.child.is_some() {
            let _ = self.send_command(&serde_json::json!(["set_property", "volume", self.volume]));
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
                self.stop_helpers();
                let _ = std::fs::remove_file(&self.ipc_path);
                return true;
            }
        }
        let idle = self.live.idle.load(Ordering::Relaxed);
        let eof = self.live.eof.load(Ordering::Relaxed);
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
            self.vis_cmd(&serde_json::json!(["seek", seconds, "absolute"]));
            self.live.stamp_pos(seconds.max(0.0));
            if let Ok(mut pcm) = self.live.pcm.lock() {
                pcm.clear();
            }
        }
        Ok(())
    }

    fn position(&mut self) -> Option<f64> {
        self.live.time_pos()
    }

    fn duration(&mut self) -> Option<f64> {
        self.live.duration()
    }

    fn stream_quality(&mut self) -> StreamQuality {
        let samplerate = match self.live.samplerate.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n),
        };
        let channels = match self.live.channels.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n as u8),
        };
        let bit_depth = match self.live.bit_depth.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n as u8),
        };
        let bitrate = match self.live.bitrate.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n),
        };
        let codec = self.live.codec.lock().ok().and_then(|g| g.clone());
        let format = self.live.format.lock().ok().and_then(|g| g.clone());
        StreamQuality {
            audio_quality: None,
            mime_type: None,
            codecs: codec,
            sample_rate_hz: samplerate,
            bit_depth: bit_depth
                .or_else(|| format.as_deref().and_then(format::bit_depth_from_format)),
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
            self.vis_cmd(&serde_json::json!(["set_property", "loop-file", val]));
        }
        Ok(())
    }

    fn drain_pcm(&mut self, dst: &mut Vec<f32>) {
        dst.clear();
        if let Ok(mut pcm) = self.live.pcm.lock() {
            dst.extend_from_slice(&pcm);
            pcm.clear();
        }
    }
}

impl Drop for MpvBackend {
    fn drop(&mut self) {
        self.kill_child();
    }
}

fn which_mpv() -> Option<PathBuf> {
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

fn unique_path(kind: &str, ext: &str) -> PathBuf {
    let n = IPC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("tiders-{kind}-{pid}-{n}.{ext}"))
}

#[cfg(unix)]
fn mkfifo(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let cstr = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "fifo path contains NUL")
    })?;
    let rc = unsafe { libc::mkfifo(cstr.as_ptr(), 0o600) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn spawn_observer(ipc: PathBuf, live: Arc<Live>, gen: Arc<AtomicU64>, mine: u64) {
    std::thread::Builder::new()
        .name("tiders-mpv-ipc".into())
        .spawn(move || observer_loop(ipc, live, gen, mine))
        .ok();
}

fn observer_loop(ipc: PathBuf, live: Arc<Live>, gen: Arc<AtomicU64>, mine: u64) {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        let mut connected: Option<UnixStream> = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && gen.load(Ordering::Relaxed) == mine {
            match UnixStream::connect(&ipc) {
                Ok(s) => {
                    connected = Some(s);
                    break;
                }
                Err(_) => std::thread::sleep(Duration::from_millis(30)),
            }
        }
        let Some(mut stream) = connected else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
        let observes = [
            serde_json::json!(["observe_property", 1, "time-pos"]),
            serde_json::json!(["observe_property", 2, "duration"]),
            serde_json::json!(["observe_property", 3, "eof-reached"]),
            serde_json::json!(["observe_property", 4, "idle-active"]),
            serde_json::json!(["observe_property", 5, "audio-params"]),
            serde_json::json!(["observe_property", 6, "audio-codec-name"]),
            serde_json::json!(["observe_property", 7, "audio-bitrate"]),
        ];
        for cmd in &observes {
            let req_id = REQ_COUNTER.fetch_add(1, Ordering::Relaxed);
            let payload = serde_json::json!({"command": cmd, "request_id": req_id});
            if let Ok(mut line) = serde_json::to_vec(&payload) {
                line.push(b'\n');
                if stream.write_all(&line).is_err() {
                    return;
                }
            }
        }
        let mut reader = BufReader::new(stream);
        let mut buf = String::new();
        while gen.load(Ordering::Relaxed) == mine {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(buf.trim()) {
                        apply_ipc_event(&live, &v);
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(_) => break,
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (ipc, live, gen, mine);
    }
}

fn apply_ipc_event(live: &Live, v: &serde_json::Value) {
    let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
    if event == "end-file" {
        if v.get("reason").and_then(|r| r.as_str()) == Some("eof") {
            live.eof.store(true, Ordering::Relaxed);
        }
        return;
    }
    if event != "property-change" {
        return;
    }
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let data = v.get("data");
    match name {
        "time-pos" => {
            if let Some(p) = data.and_then(json_f64) {
                live.stamp_pos(p);
            }
        }
        "duration" => {
            if let Some(p) = data.and_then(json_f64) {
                live.duration.store(p.to_bits(), Ordering::Relaxed);
            }
        }
        "eof-reached" => {
            live.eof.store(
                data.and_then(|d| d.as_bool()).unwrap_or(false),
                Ordering::Relaxed,
            );
        }
        "idle-active" => {
            live.idle.store(
                data.and_then(|d| d.as_bool()).unwrap_or(false),
                Ordering::Relaxed,
            );
        }
        "audio-codec-name" => {
            if let Ok(mut g) = live.codec.lock() {
                *g = data.and_then(|d| d.as_str().map(str::to_string));
            }
        }
        "audio-bitrate" => {
            if let Some(b) = data.and_then(|d| d.as_f64()) {
                live.bitrate.store(b.max(0.0) as u32, Ordering::Relaxed);
            }
        }
        "audio-params" => {
            if let Some(obj) = data.and_then(|d| d.as_object()) {
                if let Some(sr) = obj.get("samplerate").and_then(|v| v.as_u64()) {
                    live.samplerate.store(sr as u32, Ordering::Relaxed);
                }
                if let Some(ch) = obj
                    .get("channel-count")
                    .and_then(|v| v.as_u64())
                    .or_else(|| obj.get("channels").and_then(|v| v.as_u64()))
                {
                    live.channels.store(ch as u32, Ordering::Relaxed);
                }
                if let Some(fmt) = obj.get("format").and_then(|v| v.as_str()) {
                    if let Ok(mut g) = live.format.lock() {
                        *g = Some(fmt.to_string());
                    }
                    if let Some(bits) = format::bit_depth_from_format(fmt) {
                        live.bit_depth.store(bits as u32, Ordering::Relaxed);
                    }
                }
            }
        }
        _ => {}
    }
}

fn json_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_u64().map(|u| u as f64))
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn spawn_pcm_reader(fifo: PathBuf, live: Arc<Live>, gen: Arc<AtomicU64>, mine: u64) {
    std::thread::Builder::new()
        .name("tiders-pcm".into())
        .spawn(move || pcm_loop(fifo, live, gen, mine))
        .ok();
}

fn pcm_loop(fifo: PathBuf, live: Arc<Live>, gen: Arc<AtomicU64>, mine: u64) {
    let mut raw = vec![0u8; 2048];
    while gen.load(Ordering::Relaxed) == mine {
        let file = loop {
            if gen.load(Ordering::Relaxed) != mine {
                return;
            }
            match std::fs::File::open(&fifo) {
                Ok(f) => break f,
                Err(_) => std::thread::sleep(Duration::from_millis(40)),
            }
        };
        let mut reader = std::io::BufReader::new(file);
        while gen.load(Ordering::Relaxed) == mine {
            match reader.read(&mut raw) {
                Ok(0) => break,
                Ok(n) => {
                    let mut decoded = Vec::with_capacity(n / 2);
                    for chunk in raw[..n].chunks_exact(2) {
                        let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                        decoded.push(s as f32 / 32768.0);
                    }
                    if decoded.is_empty() {
                        continue;
                    }
                    if let Ok(mut pcm) = live.pcm.lock() {
                        pcm.extend_from_slice(&decoded);
                        // Keep a little more than one FFT window so the UI can drain.
                        let max = 8192;
                        if pcm.len() > max {
                            let skip = pcm.len() - max;
                            pcm.drain(..skip);
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }
}

#[cfg(unix)]
fn send_ipc(
    path: &Path,
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
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
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
    _path: &Path,
    _command: &serde_json::Value,
    _wait: Option<Duration>,
) -> Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

#[cfg(all(test, unix))]
mod tests {
    use super::mkfifo;
    use std::os::unix::fs::FileTypeExt;

    #[test]
    fn mkfifo_creates_a_named_pipe() {
        let path = std::env::temp_dir().join(format!(
            "tiders-mkfifo-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);
        mkfifo(&path).expect("libc mkfifo");
        let meta = std::fs::metadata(&path).expect("stat fifo");
        assert!(meta.file_type().is_fifo());
        let _ = std::fs::remove_file(&path);
    }
}
