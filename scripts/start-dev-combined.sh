#!/usr/bin/env bash
# Iterative development: run the server wizard app and the player together
# in Tauri dev mode, so both UIs hot-reload and neither waits on a slow
# release build.
#
#   scripts/start-dev-combined.sh
#   scripts/start-dev-combined.sh --features encode-opus,encode-mp3   # server only
#
# kahawai-server is itself a Tauri app on macOS (crates/kahawai-server,
# wizard UI dev server on :1421) — not a separate headless binary — and it
# autostarts the real axum backend in-process once a usable config exists.
# The player is a second Tauri app (player/, UI dev server on :1420).
# `cargo tauri dev` is run for each, so both get Vite hot reload for their UI
# and fast incremental (debug) Rust builds. The player starts once the
# server answers /api/health (KAHAWAI_SERVER_URL, default
# http://127.0.0.1:8080; give up after KAHAWAI_SERVER_WAIT_SECS, default 180).
# Ctrl+C stops both.
#
# macOS only: kahawai-server's Tauri shell (and this workflow) don't exist
# on other platforms — see the cfg(target_os = "macos") split in
# crates/kahawai-server/src/main.rs. Elsewhere, run the server headlessly
# with scripts/start-server.sh and the player with scripts/start-client.sh --dev.
set -euo pipefail
set -m  # each background job gets its own process group, so we can kill it (and its children) as a unit

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Install a UI's npm dependencies when they're missing or out of date:
# package.json or package-lock.json changed since the last install (a merge
# that added a dependency, say). node_modules/.package-lock.json is npm's
# record of the last install; it's touched afterwards so an install with
# nothing to do doesn't repeat on every run.
ensure_npm_deps() {
  local ui="$1"
  local stamp="${ui}/node_modules/.package-lock.json"
  if [[ ! -f "${stamp}" || "${ui}/package.json" -nt "${stamp}" || "${ui}/package-lock.json" -nt "${stamp}" ]]; then
    echo "==> npm install in ${ui#"${ROOT}"/} (dependencies missing or changed)…"
    (cd "${ui}" && npm install --no-audit --no-fund)
    touch "${stamp}"
  fi
}

# Both apps' dev servers (beforeDevCommand: npm run dev) need their npm
# dependencies installed and current.
ensure_npm_deps "${ROOT}/crates/kahawai-server/ui"
ensure_npm_deps "${ROOT}/player/ui"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: kahawai-server's Tauri wizard UI is macOS-only (this is $(uname -s))." >&2
  echo "Use scripts/start-server.sh for the headless server and" >&2
  echo "scripts/start-client.sh --dev for the player." >&2
  exit 1
fi

command -v cargo-tauri >/dev/null 2>&1 || { echo "error: tauri-cli missing. Install: cargo install tauri-cli" >&2; exit 1; }

features=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --features) features="${2:?--features needs a list}"; shift 2 ;;
    -h|--help)  sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 1 ;;
  esac
done
feat_args=()
[[ -n "$features" ]] && feat_args=(-f "$features")

# `cargo tauri dev` for each app starts its own Vite dev server (server
# wizard: :1421, player: :1420) with strict ports. A leftover Vite from an
# earlier session would make it fail; stop a stale one, refuse to touch
# anything else.
stop_stale_vite() {
  local port="$1" pid cmd
  for pid in $(lsof -nP -iTCP:"$port" -sTCP:LISTEN -t 2>/dev/null | sort -u); do
    cmd="$(ps -o command= -p "$pid" 2>/dev/null || true)"
    if [[ "$cmd" == *vite* ]]; then
      echo "Stopping a leftover Vite dev server on :$port (pid $pid)"
      kill "$pid" 2>/dev/null || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 0.3; done
    else
      echo "error: port $port (a dev server port) is in use by: $cmd" >&2
      exit 1
    fi
  done
}
stop_stale_vite 1421
stop_stale_vite 1420

SERVER_LOG="$ROOT/kahawai-server-dev.log"
: >"$SERVER_LOG"

server_pgid=""
cleanup() {
  if [[ -n "$server_pgid" ]]; then
    echo
    echo "Stopping server dev (Kahawai Server wizard app)…"
    kill -TERM -- "-$server_pgid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 -- "-$server_pgid" 2>/dev/null || break; sleep 0.5; done
    kill -KILL -- "-$server_pgid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

echo "==> starting Kahawai Server wizard app in dev mode (cargo tauri dev, :1421)…"
(
  cd "$ROOT/crates/kahawai-server"
  exec cargo tauri dev ${feat_args[@]+"${feat_args[@]}"}
) >>"$SERVER_LOG" 2>&1 &
server_pgid=$!
echo "(logging to $SERVER_LOG)"

# Give the server a head start: the player loads the library at launch, so
# it should find the backend already listening. Both builds share cargo's
# lock anyway, so waiting costs little. Not fatal on timeout: the player
# reloads the library as soon as the server's event stream connects.
SERVER_URL="${KAHAWAI_SERVER_URL:-http://127.0.0.1:8080}"
WAIT_SECS="${KAHAWAI_SERVER_WAIT_SECS:-180}"
echo "==> waiting up to ${WAIT_SECS}s for the server at ${SERVER_URL}…"
waited=0
until curl -sf -o /dev/null --max-time 2 "$SERVER_URL/api/health"; do
  if ! kill -0 -- "-$server_pgid" 2>/dev/null; then
    echo "warning: the server dev process exited; see $SERVER_LOG" >&2
    break
  fi
  if (( waited >= WAIT_SECS )); then
    echo "note: no answer from $SERVER_URL after ${WAIT_SECS}s (still building, or waiting in its setup wizard); starting the player anyway." >&2
    break
  fi
  sleep 2
  waited=$((waited + 2))
done

echo "==> starting Kahawai Player in dev mode (cargo tauri dev, :1420)…"
cd "$ROOT/player"
cargo tauri dev
