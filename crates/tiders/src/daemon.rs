//! Headless engine: Unix IPC + in-process MPRIS.
//!
//! Front-ends (Noctalia, `tiders ctl`, later a Sendspin adapter) attach to
//! the socket; this process owns the TIDAL session and the mpv backend.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::{broadcast, mpsc};

use tiders_core::config::{Config, Settings};
use tiders_core::engine::ipc::{self, IpcJob, IpcReply};
use tiders_core::engine::{Engine, EngineCommand, EngineEvent, PROTOCOL_VERSION};
use tiders_core::spectrum::SpectrumFrame;

/// Bind the control socket and run until SIGINT/SIGTERM or a `quit` command.
pub async fn run(config: Config, settings: Settings, socket: Option<PathBuf>) -> Result<()> {
    let path = socket.unwrap_or_else(ipc::socket_path);
    let listener = ipc::bind(&path).context("binding IPC socket")?;
    println!("tiders daemon listening on {}", path.display());
    println!("  ctl:  tiders ctl --socket {} status", path.display());
    println!("  roles: controller, metadata, artwork, visualizer, player");
    println!("  transports: ipc, mpris  (sendspin / music-assistant: later adapters)");

    let mut engine = Engine::start(config, settings)
        .await
        .context("starting engine (are you signed in?)")?;

    let (job_tx, mut job_rx) = mpsc::unbounded_channel::<IpcJob>();
    let (ev_tx, _) = broadcast::channel::<EngineEvent>(64);
    tokio::spawn(ipc::serve(listener, job_tx, ev_tx.clone()));

    let _ = ev_tx.send(engine.state_event());

    let mut ticker = tokio::time::interval(Duration::from_millis(33));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last = std::time::Instant::now();
    let mut last_state = std::time::Instant::now();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = engine.handle(EngineCommand::Quit).await;
                break;
            }
            job = job_rx.recv() => {
                let Some(job) = job else { break };
                let reply = dispatch(&mut engine, job.req.id, job.req.cmd, &ev_tx).await;
                let _ = job.reply.send(reply);
                if engine.should_quit {
                    break;
                }
            }
            _ = ticker.tick() => {
                let now = std::time::Instant::now();
                let dt = now.saturating_duration_since(last).as_secs_f32();
                last = now;
                let frame = engine.tick(dt).await;
                if last_state.elapsed() >= Duration::from_millis(250) {
                    let _ = ev_tx.send(engine.state_event());
                    last_state = now;
                }
                if let Some(SpectrumFrame { bars, peaks }) = frame {
                    let _ = ev_tx.send(EngineEvent::Spectrum { bars, peaks });
                }
            }
        }
    }

    let _ = std::fs::remove_file(&path);
    Ok(())
}

async fn dispatch(
    engine: &mut Engine,
    id: u64,
    cmd: EngineCommand,
    ev_tx: &broadcast::Sender<EngineEvent>,
) -> IpcReply {
    match engine.handle(cmd).await {
        Ok(_) => {
            let state = engine.snapshot();
            let _ = ev_tx.send(EngineEvent::State {
                state: Box::new(state.clone()),
            });
            IpcReply {
                v: PROTOCOL_VERSION,
                id,
                ok: true,
                error: None,
                state: Some(state),
            }
        }
        Err(e) => {
            let message = e.to_string();
            let _ = ev_tx.send(EngineEvent::Error {
                message: message.clone(),
            });
            IpcReply {
                v: PROTOCOL_VERSION,
                id,
                ok: false,
                error: Some(message),
                state: None,
            }
        }
    }
}
