#!/usr/bin/env bash
# Idempotent setup for the Tiders development environment.
#
#   • installs mpv (the runtime audio backend) if missing
#   • ensures a Rust toolchain that supports edition 2024 (>= 1.85)
#     — pinned to `stable` via rust-toolchain.toml
#   • warms the cargo dependency cache and builds the workspace
#
# Safe to run repeatedly (during setup, snapshots, and re-runs).
set -euo pipefail

echo "== Tiders environment setup =="

# ── System packages ─────────────────────────────────────────────────────────
# mpv plays the resolved TIDAL streams (FLAC/DASH/HLS). Only needed at runtime
# for actual audio; the rest of the app works without it.
if ! command -v mpv >/dev/null 2>&1; then
  echo "Installing mpv…"
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq mpv
fi
echo "mpv: $(mpv --version 2>/dev/null | head -n1 || echo 'not installed')"

# ── Rust toolchain ──────────────────────────────────────────────────────────
# rust-toolchain.toml pins `stable`; make sure it (and clippy/rustfmt) exist.
rustup toolchain install stable --profile minimal -c clippy -c rustfmt
rustup show

# ── Warm caches + build ─────────────────────────────────────────────────────
# Fetch git + crates.io dependencies (incl. tidlers from Codeberg), then build
# so a fresh agent starts ready to run/test immediately.
if [ -f Cargo.lock ]; then
  cargo fetch --locked || cargo fetch
else
  cargo fetch
fi
cargo build

echo "== Tiders setup complete =="
