#!/usr/bin/env bash
# Run the player on the iOS simulator in Tauri dev mode: the UI hot-reloads,
# Rust changes rebuild and reinstall the app.
#
#   scripts/start-dev-ios.sh
#   scripts/start-dev-ios.sh --device "Kyo's iPhone"   # a connected device
#
# Steps, each skipped when already done:
#   1. Xcode present (macOS only).
#   2. npm dependencies for player/ui.
#   3. `cargo tauri ios init` when player/src-tauri/gen/ios is missing
#      (generated, gitignored; never commit it).
#   4. scripts/ios-plist-overlay.py (background-audio mode).
#   5. `cargo tauri ios dev`: builds the Rust core for the simulator (or the
#      named device), installs the app, and serves the UI from Vite on :1420.
#      Ctrl+C stops it.
#
# The server: the simulator shares this Mac's network, so the player's
# Settings can use http://localhost:8080 with the server running on the Mac
# (scripts/start-server.sh). A physical device needs the Mac's LAN address.
#
# NOTE: not yet run — no Apple toolchain in this environment. Expect small
# fixes on first use; keep them in this file.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

device=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --device) device="${2:?--device needs a device name}"; shift 2 ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \\{0,1\\}//'; exit 0 ;;
    *) echo "error: unknown argument '$1' (try --help)" >&2; exit 1 ;;
  esac
done

[[ "$(uname)" == "Darwin" ]] || { echo "error: iOS dev needs macOS with Xcode." >&2; exit 1; }
command -v xcodebuild >/dev/null 2>&1 || { echo "error: Xcode not found." >&2; exit 1; }
command -v cargo-tauri >/dev/null 2>&1 || { echo "error: tauri-cli missing. Install: cargo install tauri-cli" >&2; exit 1; }

# The UI's npm dependencies, installed when missing or out of date.
stamp="$ROOT/player/ui/node_modules/.package-lock.json"
if [[ ! -f "$stamp" || "$ROOT/player/ui/package.json" -nt "$stamp" || "$ROOT/player/ui/package-lock.json" -nt "$stamp" ]]; then
  echo "==> npm install in player/ui (dependencies missing or changed)…"
  (cd "$ROOT/player/ui" && npm install --no-audit --no-fund)
  touch "$stamp"
fi

cd "$ROOT/player"
if [[ ! -d src-tauri/gen/ios ]]; then
  echo "==> Generating the iOS project (cargo tauri ios init)…"
  cargo tauri ios init --ci
fi

python3 "$ROOT/scripts/ios-plist-overlay.py"

dev_args=()
if [[ -n "$device" ]]; then
  dev_args=(--device "$device")
fi
echo "==> cargo tauri ios dev ${dev_args[*]}"
cd "$ROOT/player/src-tauri"
cargo tauri ios dev "${dev_args[@]}"
