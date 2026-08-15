//! Tiders — a terminal (TUI + CLI) client for the TIDAL music streaming service.
//!
//! Running `tiders` with no arguments launches the interactive TUI; subcommands
//! (`login`, `search`, `play`, …) provide scriptable access to the same
//! [`tiders_core`] engine.

mod cli;
mod daemon;
mod output;
mod tui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
