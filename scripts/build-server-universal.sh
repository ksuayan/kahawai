#!/usr/bin/env bash
# Build a macOS Universal Binary (x86_64 + arm64) of kahawai-server via lipo.
#
# MAC-ONLY: this script refuses to run on any other OS. Producing a universal
# binary requires macOS (for lipo and the Apple SDKs). On Linux CI/VMs, use
# `cargo check --workspace` / `cargo test --workspace` to validate the code.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: universal builds require macOS (this is $(uname -s))." >&2
  echo "On Linux, validate with: cargo check --workspace && cargo test --workspace" >&2
  exit 1
fi

for target in x86_64-apple-darwin aarch64-apple-darwin; do
  if ! rustup target list --installed | grep -q "^${target}$"; then
    echo "error: Rust target '${target}' is not installed." >&2
    echo "Install with: rustup target add ${target}" >&2
    exit 1
  fi
done

cd "$(dirname "$0")/.."

echo "==> building x86_64 (Intel)…"
cargo build --release --target x86_64-apple-darwin -p kahawai-server
echo "==> building aarch64 (Apple Silicon)…"
cargo build --release --target aarch64-apple-darwin -p kahawai-server

OUT="dist/kahawai-server"
mkdir -p dist
echo "==> lipo → ${OUT}"
lipo -create \
  target/x86_64-apple-darwin/release/kahawai-server \
  target/aarch64-apple-darwin/release/kahawai-server \
  -output "${OUT}"

echo "==> verifying"
lipo -archs "${OUT}"
file "${OUT}"
echo "done: ${OUT}"
