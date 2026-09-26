#!/usr/bin/env bash
# Launch Kahawai Player once the server is up.
#
#   scripts/start-client.sh                 # use the server URL saved by setup.sh / the app
#   scripts/start-client.sh --url http://192.168.1.10:8080   # also saved into the app's settings
#   scripts/start-client.sh --wait 60       # wait up to 60s for the server (default 30)
#   scripts/start-client.sh --dev           # run `cargo tauri dev` instead of a built app
#
# Picks the first app it finds: dist/Kahawai Player.app (universal build),
# then a per-arch bundle under player/src-tauri/target. Falls back to dev mode.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="Kahawai Player"
APP_ID="com.suayan.kahawai-player"

url="" wait_secs=30 dev=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --url)  url="${2:?--url needs a value}"; shift 2 ;;
    --wait) wait_secs="${2:?--wait needs a value}"; shift 2 ;;
    --dev)  dev=1; shift ;;
    -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 1 ;;
  esac
done

case "$(uname -s)" in
  Darwin) settings="$HOME/Library/Application Support/$APP_ID/engine-settings.json" ;;
  *)      settings="${XDG_CONFIG_HOME:-$HOME/.config}/$APP_ID/engine-settings.json" ;;
esac

# Server URL: flag > saved client settings > default.
url_from_flag=0
[[ -n "$url" ]] && url_from_flag=1
if [[ -z "$url" ]]; then
  if [[ -f "$settings" ]] && command -v python3 >/dev/null 2>&1; then
    url="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("server_url",""))' "$settings" 2>/dev/null || true)"
  fi
  url="${url:-http://localhost:8080}"
fi
url="${url%/}"

# Wait for the server.
command -v curl >/dev/null 2>&1 || { echo "error: curl is required" >&2; exit 1; }
printf 'Waiting for server at %s ' "$url"
deadline=$(( $(date +%s) + wait_secs ))
until health="$(curl -fsS --max-time 2 "$url/api/health" 2>/dev/null)"; do
  if (( $(date +%s) >= deadline )); then
    echo
    echo "error: no server at $url after ${wait_secs}s." >&2
    echo "Start it first, e.g.: cargo run --release -p kahawai-server -- config.toml" >&2
    echo "(or pass --url / --wait)." >&2
    exit 1
  fi
  printf '.'
  sleep 1
done
echo
echo "Server is up: $health"

# The app reads its server URL from its own settings file, so a --url flag
# must be saved there or the app keeps using the old address.
if [[ "$url_from_flag" -eq 1 ]]; then
  command -v python3 >/dev/null 2>&1 || { echo "error: python3 is required to save --url into the app settings" >&2; exit 1; }
  mkdir -p "$(dirname "$settings")"
  python3 - "$settings" "$url" <<'PYEOF'
import json, sys
path, url = sys.argv[1:3]
try:
    with open(path) as f:
        cfg = json.load(f)
except Exception:
    cfg = {}
if cfg.get("server_url") != url:
    cfg["server_url"] = url
    with open(path, "w") as f:
        json.dump(cfg, f, indent=2)
        f.write("\n")
    print("Saved server URL to app settings: " + url)
PYEOF
  if pgrep -f "$APP_NAME.app/Contents/MacOS" >/dev/null 2>&1; then
    echo "warning: $APP_NAME is already running and keeps its old URL until you quit and relaunch it." >&2
  fi
fi

# Find something to launch.
app=""
if [[ "$dev" -eq 0 ]]; then
  for c in \
    "$ROOT/dist/$APP_NAME.app" \
    "$ROOT"/player/src-tauri/target/*/release/bundle/macos/"$APP_NAME.app" \
    "$ROOT/player/src-tauri/target/release/bundle/macos/$APP_NAME.app"; do
    [[ -d "$c" ]] && { app="$c"; break; }
  done
fi

if [[ -n "$app" ]]; then
  echo "Launching $app"
  open "$app"
else
  [[ "$dev" -eq 1 ]] || echo "No built app found (run scripts/build-client-universal.sh); using dev mode."
  command -v cargo-tauri >/dev/null 2>&1 || { echo "error: tauri-cli missing. Install: cargo install tauri-cli" >&2; exit 1; }
  # `cargo tauri dev` starts its own Vite dev server on :1420 (strict port). A
  # Vite left behind by an earlier dev session would make it fail, or leave
  # two running. Stop a stale Vite; refuse to touch anything else.
  for pid in $(lsof -nP -iTCP:1420 -sTCP:LISTEN -t 2>/dev/null | sort -u); do
    cmd="$(ps -o command= -p "$pid" 2>/dev/null || true)"
    if [[ "$cmd" == *vite* ]]; then
      echo "Stopping a leftover Vite dev server on :1420 (pid $pid)"
      kill "$pid" 2>/dev/null || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 0.3; done
    else
      echo "error: port 1420 (the dev server port) is in use by: $cmd" >&2
      exit 1
    fi
  done
  cd "$ROOT/player"
  exec cargo tauri dev
fi
