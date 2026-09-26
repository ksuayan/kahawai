#!/usr/bin/env bash
# Start Kahawai Server.
#
#   scripts/start-server.sh                  # run in the foreground (Ctrl+C stops it)
#   scripts/start-server.sh -d               # run in the background (log + pid file)
#   scripts/start-server.sh --stop           # stop the background server
#   scripts/start-server.sh --status         # is it running?
#   scripts/start-server.sh -c my.toml       # use another config (default: config.toml)
#   scripts/start-server.sh --build          # (re)build first; also: --features encode-opus,encode-mp3
#
# Runs the newest server binary of target/release (native build) and dist/
# (universal build). If there is none it builds one (native, release). An existing binary is NOT rebuilt
# unless you pass --build, so a build made with --features is never silently
# replaced by a plain one.
#
# The server has no authentication and no TLS: bind it to a private LAN address.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PID_FILE="$ROOT/.kahawai-server.pid"
LOG_FILE="$ROOT/kahawai-server.log"

config="config.toml" background=0 build=0 features="" action="start"
while [[ $# -gt 0 ]]; do
  case "$1" in
    -c|--config)   config="${2:?--config needs a path}"; shift 2 ;;
    -d|--background) background=1; shift ;;
    --build)       build=1; shift ;;
    --features)    features="${2:?--features needs a list}"; build=1; shift 2 ;;
    --stop)        action="stop"; shift ;;
    --status)      action="status"; shift ;;
    -h|--help)     sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 1 ;;
  esac
done

running_pid() {
  [[ -f "$PID_FILE" ]] || return 1
  local pid
  pid="$(cat "$PID_FILE" 2>/dev/null || true)"
  [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null && { echo "$pid"; return 0; }
  rm -f "$PID_FILE"  # stale
  return 1
}

# Value of a top-level `key = "value"` in the TOML config (empty if absent).
cfg_get() {
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\(.*\)\"[[:space:]]*\$/\1/p" "$config" | head -1
}

case "$action" in
  stop)
    if pid="$(running_pid)"; then
      kill "$pid"
      for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
      if kill -0 "$pid" 2>/dev/null; then kill -9 "$pid" 2>/dev/null || true; fi
      rm -f "$PID_FILE"
      echo "Stopped server (pid $pid)."
    else
      echo "No background server is running."
    fi
    exit 0 ;;
  status)
    if pid="$(running_pid)"; then
      echo "Server is running (pid $pid). Log: $LOG_FILE"
    else
      echo "Server is not running (from this script)."
      exit 1
    fi
    exit 0 ;;
esac

if pid="$(running_pid)"; then
  echo "error: a background server is already running (pid $pid). Stop it with --stop." >&2
  exit 1
fi

# -- config
if [[ ! -f "$config" ]]; then
  echo "No config found at $config."
  if [[ -t 0 && -x "$ROOT/scripts/setup.sh" && "$config" == "config.toml" ]]; then
    read -r -p "Run the setup wizard now? [Y/n]: " ans
    case "$(printf '%s' "${ans:-y}" | tr '[:upper:]' '[:lower:]')" in
      y|yes) "$ROOT/scripts/setup.sh" server ;;
    esac
  fi
  [[ -f "$config" ]] || { echo "error: create $config first (scripts/setup.sh server)." >&2; exit 1; }
fi

bind="$(cfg_get bind)"; bind="${bind:-0.0.0.0:8080}"
port="${bind##*:}"
music_dirs_line="$(grep -E '^[[:space:]]*music_dirs' "$config" || true)"
if [[ "$music_dirs_line" == *"[]"* || -z "$music_dirs_line" ]]; then
  echo "warning: no music_dirs in $config; there will be nothing to scan." >&2
fi

# -- binary
# The newest of the native build (target/release) and the universal build
# (dist/), so a fresh build of either is what runs.
find_bin() {
  local best="" c
  for c in target/release/kahawai-server dist/kahawai-server; do
    [[ -x "$c" ]] || continue
    if [[ -z "$best" || "$c" -nt "$best" ]]; then best="$c"; fi
  done
  [[ -n "$best" ]] && echo "$best"
}

if [[ "$build" -eq 1 ]] || ! bin="$(find_bin)"; then
  command -v cargo >/dev/null 2>&1 || { echo "error: cargo not found and no built server. Install Rust (https://rustup.rs)." >&2; exit 1; }
  echo "==> building kahawai-server (release)${features:+ with features: $features}…"
  if [[ -n "$features" ]]; then
    cargo build --release -p kahawai-server --features "$features"
  else
    cargo build --release -p kahawai-server
  fi
  bin="target/release/kahawai-server"
fi
if [[ -x target/release/kahawai-server && -x dist/kahawai-server ]]; then
  echo "(using the newest of target/release and dist/: $bin)"
fi

# -- port check (best effort)
if command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "error: something is already listening on port $port:" >&2
  lsof -nP -iTCP:"$port" -sTCP:LISTEN >&2 || true
  exit 1
fi

url="http://${bind/0.0.0.0/localhost}"
echo "Starting $bin with $config  →  $url"
echo "(no authentication, no TLS: trusted LAN only)"

if [[ "$background" -eq 1 ]]; then
  nohup "$bin" "$config" >>"$LOG_FILE" 2>&1 &
  echo $! > "$PID_FILE"
  # Confirm it came up before reporting success.
  for _ in $(seq 1 20); do
    if curl -fsS --max-time 1 "$url/api/health" >/dev/null 2>&1; then
      echo "Server is up (pid $(cat "$PID_FILE")). Log: $LOG_FILE"
      echo "Stop it with: scripts/start-server.sh --stop"
      exit 0
    fi
    kill -0 "$(cat "$PID_FILE")" 2>/dev/null || break
    sleep 0.5
  done
  echo "error: server did not come up. Last log lines:" >&2
  tail -n 15 "$LOG_FILE" >&2 || true
  rm -f "$PID_FILE"
  exit 1
fi

exec "$bin" "$config"
