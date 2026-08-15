//! Local JSON-over-Unix-socket transport.
//!
//! Newline-delimited JSON, one object per line — the same shape a Noctalia
//! plugin, `tiders ctl`, or a future Sendspin adapter can speak:
//!
//! ```json
//! {"v":1,"id":1,"cmd":"play-pause"}
//! {"v":1,"id":1,"ok":true,"state":{...}}
//! {"v":1,"event":"state","state":{...}}
//! ```
//!
//! The socket lives at `$XDG_RUNTIME_DIR/tiders.sock` (override with
//! `TIDERS_SOCK` or `--socket`). Only one daemon binds it.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::error::{Error, Result};

use super::{EngineCommand, EngineEvent, EngineState, PROTOCOL_VERSION};

/// Environment variable that overrides the IPC socket path.
pub const SOCK_ENV: &str = "TIDERS_SOCK";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    #[serde(default = "one")]
    pub v: u32,
    #[serde(default)]
    pub id: u64,
    #[serde(flatten)]
    pub cmd: EngineCommand,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcReply {
    pub v: u32,
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<EngineState>,
}

pub struct IpcJob {
    pub req: IpcRequest,
    pub reply: oneshot::Sender<IpcReply>,
}

/// Resolve the control socket path.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os(SOCK_ENV) {
        return PathBuf::from(p);
    }
    if let Some(dir) = dirs::runtime_dir() {
        return dir.join("tiders.sock");
    }
    let uid = {
        #[cfg(unix)]
        {
            unsafe { libc::geteuid() }
        }
        #[cfg(not(unix))]
        {
            0
        }
    };
    std::env::temp_dir().join(format!("tiders-{uid}.sock"))
}

/// Bind the listener, replacing a stale socket left by a crashed daemon.
pub fn bind(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        // If something is already listening, fail loudly instead of stealing.
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(Error::other(format!(
                "tiders daemon already running at {}",
                path.display()
            )));
        }
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let listener = UnixListener::bind(path).map_err(|e| Error::io(path, e))?;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        let _ = std::fs::remove_file(path);
        return Err(Error::io(path, e));
    }
    Ok(listener)
}

/// Accept loop: each connection gets hello + optional event stream.
pub async fn serve(
    listener: UnixListener,
    jobs: mpsc::UnboundedSender<IpcJob>,
    events: broadcast::Sender<EngineEvent>,
) {
    while let Ok((stream, _)) = listener.accept().await {
        let jobs = jobs.clone();
        let events = events.subscribe();
        tokio::spawn(connection(stream, jobs, events));
    }
}

async fn connection(
    stream: UnixStream,
    jobs: mpsc::UnboundedSender<IpcJob>,
    mut events: broadcast::Receiver<EngineEvent>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();

    let hello = encode_event(&EngineEvent::hello());
    let _ = out_tx.send(hello);

    let write = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    let mut want_spectrum = false;
    loop {
        tokio::select! {
            biased;
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) if !line.trim().is_empty() => {
                        match serde_json::from_str::<IpcRequest>(&line) {
                            Ok(req) => {
                                if let EngineCommand::Subscribe { spectrum } = &req.cmd {
                                    want_spectrum = *spectrum;
                                }
                                let (tx, rx) = oneshot::channel();
                                if jobs.send(IpcJob { req, reply: tx }).is_err() {
                                    break;
                                }
                                if let Ok(reply) = rx.await {
                                    if let Ok(json) = serde_json::to_string(&reply) {
                                        let _ = out_tx.send(json);
                                    }
                                }
                            }
                            Err(e) => {
                                let reply = IpcReply {
                                    v: PROTOCOL_VERSION,
                                    id: 0,
                                    ok: false,
                                    error: Some(format!("bad request: {e}")),
                                    state: None,
                                };
                                if let Ok(json) = serde_json::to_string(&reply) {
                                    let _ = out_tx.send(json);
                                }
                            }
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break,
                }
            }
            ev = events.recv() => {
                match ev {
                    Ok(EngineEvent::Spectrum { .. }) if !want_spectrum => {}
                    Ok(ev) => {
                        let _ = out_tx.send(encode_event(&ev));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    drop(out_tx);
    let _ = write.await;
}

fn encode_event(ev: &EngineEvent) -> String {
    #[derive(Serialize)]
    struct Frame<'a> {
        v: u32,
        #[serde(flatten)]
        ev: &'a EngineEvent,
    }
    serde_json::to_string(&Frame {
        v: PROTOCOL_VERSION,
        ev,
    })
    .unwrap_or_else(|_| "{\"v\":1,\"event\":\"error\",\"message\":\"encode\"}".into())
}

/// Client helper used by `tiders ctl`.
pub async fn send(path: &Path, cmd: EngineCommand) -> Result<IpcReply> {
    let stream = UnixStream::connect(path)
        .await
        .map_err(|e| Error::other(format!("connect {}: {e}", path.display())))?;
    let (reader, mut writer) = stream.into_split();
    let req = IpcRequest {
        v: PROTOCOL_VERSION,
        id: 1,
        cmd,
    };
    let mut line = serde_json::to_vec(&req)?;
    line.push(b'\n');
    writer
        .write_all(&line)
        .await
        .map_err(|e| Error::other(format!("ipc write: {e}")))?;
    writer
        .shutdown()
        .await
        .map_err(|e| Error::other(format!("ipc shutdown: {e}")))?;

    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| Error::other(format!("ipc read: {e}")))?
    {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(reply) = serde_json::from_str::<IpcReply>(&line) {
            if reply.id == 1 || !reply.ok {
                return Ok(reply);
            }
        }
        // Skip hello / unsolicited events until the matching reply.
    }
    Err(Error::other("ipc closed before reply"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_flattens_cmd() {
        let req = IpcRequest {
            v: 1,
            id: 7,
            cmd: EngineCommand::Seek { seconds: 12.5 },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"cmd\":\"seek\""));
        assert!(json.contains("\"id\":7"));
        let back: IpcRequest = serde_json::from_str(&json).unwrap();
        match back.cmd {
            EngineCommand::Seek { seconds } => assert!((seconds - 12.5).abs() < f64::EPSILON),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn ipc_roundtrip_status() {
        let path = std::env::temp_dir().join(format!(
            "tiders-ipc-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);
        let listener = bind(&path).expect("bind");
        let mode = std::fs::metadata(&path)
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "ipc socket must be owner-only");
        let (job_tx, mut job_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ev_tx, _) = tokio::sync::broadcast::channel(8);
        tokio::spawn(serve(listener, job_tx, ev_tx));
        tokio::spawn(async move {
            if let Some(job) = job_rx.recv().await {
                let _ = job.reply.send(IpcReply {
                    v: 1,
                    id: job.req.id,
                    ok: true,
                    error: None,
                    state: None,
                });
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let reply = send(&path, EngineCommand::Status).await.expect("ctl");
        assert!(reply.ok);
        let _ = std::fs::remove_file(&path);
    }
}
