#!/usr/bin/env bash
# Interactive setup wizard for Kahawai Server and Kahawai Player.
#
#   scripts/setup.sh            # choose server / client / both
#   scripts/setup.sh server
#   scripts/setup.sh client
#   scripts/setup.sh both
#
# Server: checks toolchain, writes config.toml, optionally builds + starts it.
# Client: points the desktop player at your server by writing its
#         engine-settings.json (the same file the app's Settings screen edits),
#         optionally installs the UI deps.
#
# Works on macOS (bash 3.2) and Linux. Existing files are backed up, never
# silently overwritten.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# ---------------------------------------------------------------- helpers ---
if [[ -t 1 ]]; then
  B=$'\033[1m'; D=$'\033[2m'; G=$'\033[32m'; Y=$'\033[33m'; R=$'\033[31m'; C=$'\033[36m'; N=$'\033[0m'
else
  B=""; D=""; G=""; Y=""; R=""; C=""; N=""
fi

step()  { printf '\n%s== %s ==%s\n' "$B$C" "$1" "$N"; }
info()  { printf '%s\n' "$1"; }
ok()    { printf '%s✓%s %s\n' "$G" "$N" "$1"; }
warn()  { printf '%s!%s %s\n' "$Y" "$N" "$1"; }
die()   { printf '%serror:%s %s\n' "$R" "$N" "$1" >&2; exit 1; }
have()  { command -v "$1" >/dev/null 2>&1; }
lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }

# ask "Prompt" "default"  -> sets REPLY (default used on empty input)
ask() {
  local prompt="$1" default="${2:-}" ans
  if [[ -n "$default" ]]; then
    read -r -p "$prompt [${default}]: " ans || die "input closed"
    REPLY="${ans:-$default}"
  else
    read -r -p "$prompt: " ans || die "input closed"
    REPLY="$ans"
  fi
}

# confirm "Question" "y|n"  -> returns 0 for yes
confirm() {
  local prompt="$1" default="${2:-y}" ans hint
  [[ "$default" == "y" ]] && hint="Y/n" || hint="y/N"
  while true; do
    read -r -p "$prompt [$hint]: " ans || die "input closed"
    ans="$(lower "${ans:-$default}")"
    case "$ans" in
      y|yes) return 0 ;;
      n|no)  return 1 ;;
    esac
    echo "Please answer y or n."
  done
}

# choose "Prompt" default_index option1 option2 ...  -> sets REPLY_INDEX (1-based)
choose() {
  local prompt="$1" default="$2" i n ans
  shift 2
  n=$#
  echo "$prompt"
  i=1
  for opt in "$@"; do
    printf '  %s%d)%s %s\n' "$B" "$i" "$N" "$opt"
    i=$((i + 1))
  done
  while true; do
    read -r -p "Choice [${default}]: " ans || die "input closed"
    ans="${ans:-$default}"
    if [[ "$ans" =~ ^[0-9]+$ ]] && (( ans >= 1 && ans <= n )); then
      REPLY_INDEX="$ans"
      return
    fi
    echo "Enter a number from 1 to $n."
  done
}

toml_str() {  # escape for a TOML basic string
  local s="${1//\\/\\\\}"
  printf '%s' "${s//\"/\\\"}"
}

backup_if_exists() {
  local f="$1"
  if [[ -e "$f" ]]; then
    local b="${f}.bak.$(date +%Y%m%d-%H%M%S)"
    cp -p "$f" "$b"
    warn "existing $(basename "$f") backed up to $b"
  fi
}

lan_ip() {
  local ip=""
  case "$(uname -s)" in
    Darwin)
      local iface
      iface="$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')"
      [[ -n "$iface" ]] && ip="$(ipconfig getifaddr "$iface" 2>/dev/null || true)"
      ;;
    *)
      have hostname && ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
      ;;
  esac
  printf '%s' "$ip"
}

# ----------------------------------------------------------------- server ---
setup_server() {
  step "Kahawai Server"
  cat <<EOF
The server has ${B}no authentication, no TLS and no rate limiting${N}.
Bind it to a private LAN address behind your router. Never expose it to
the internet.
EOF

  # -- toolchain
  step "1/6  Toolchain"
  have cargo || die "cargo not found. Install Rust from https://rustup.rs and re-run."
  ok "cargo $(cargo --version | awk '{print $2}')"

  # -- music dirs
  step "2/6  Music library"
  local dirs=() d
  info "Add the folders the scanner should walk (blank line to finish)."
  while true; do
    ask "Music folder #$(( ${#dirs[@]} + 1 ))" ""
    d="$REPLY"
    if [[ -z "$d" ]]; then
      (( ${#dirs[@]} > 0 )) && break
      warn "No folders yet. Scanning stays disabled until you add one."
      confirm "Continue without a music folder?" n && break
      continue
    fi
    d="${d/#\~/$HOME}"
    if [[ -d "$d" ]]; then
      d="$(cd "$d" && pwd)"
      dirs+=("$d")
      ok "added $d"
    else
      warn "$d is not a directory."
      confirm "Add it anyway (e.g. a drive that is not mounted yet)?" n && dirs+=("$d")
    fi
  done

  # -- network
  step "3/6  Network"
  local ip default_bind bind port
  ip="$(lan_ip)"
  if [[ -n "$ip" ]]; then
    info "Detected LAN address: $ip"
    default_bind="$ip:8080"
  else
    default_bind="127.0.0.1:8080"
  fi
  while true; do
    ask "Bind address (ip:port)" "$default_bind"
    bind="$REPLY"
    if [[ "$bind" =~ ^[0-9.]+:[0-9]+$ ]]; then break; fi
    echo "Use the form 192.168.1.10:8080 (IPv4 address and port)."
  done
  port="${bind##*:}"
  if [[ "${bind%%:*}" == "0.0.0.0" ]]; then
    warn "0.0.0.0 listens on every interface. Only do this behind a firewall."
  fi

  # -- storage
  step "4/6  Catalog database"
  ask "SQLite catalog path" "data/music.db"
  local db_path="$REPLY"

  # -- audio
  step "5/6  Audio"
  choose "DSD handling (server-side):" 1 \
    "pcm    - convert DSD to PCM/FLAC (works with any DAC)" \
    "native - DSD-over-PCM (DoP) for a DSD-capable DAC"
  local dsd="pcm"
  [[ "$REPLY_INDEX" == "2" ]] && dsd="native"

  choose "Preferred format ladder (first usable entry wins):" 1 \
    "passthrough, flac        (recommended for desktop)" \
    "flac                     (always transcode to FLAC)" \
    "passthrough, flac, opus  (adds Opus; needs the encode-opus build)" \
    "passthrough, flac, mp3   (adds MP3;  needs the encode-mp3 build)"
  local ladder features=""
  case "$REPLY_INDEX" in
    1) ladder='"passthrough", "flac"' ;;
    2) ladder='"flac"' ;;
    3) ladder='"passthrough", "flac", "opus"'; features="encode-opus" ;;
    4) ladder='"passthrough", "flac", "mp3"';  features="encode-mp3" ;;
  esac

  local extra=""
  if [[ -z "$features" ]]; then
    if confirm "Also build optional Opus/MP3 encoders?" n; then
      confirm "  Opus encoding?" y && extra="encode-opus"
      if confirm "  MP3 encoding?" y; then
        extra="${extra:+$extra,}encode-mp3"
      fi
    fi
    features="$extra"
  fi

  # encoder build prerequisites
  if [[ "$features" == *encode-opus* ]] && ! have cmake; then
    warn "encode-opus needs CMake (opusic-sys builds bundled libopus)."
    if [[ "$(uname -s)" == "Darwin" ]]; then info "  Install with: brew install cmake"
    else info "  Install with your package manager, e.g. apt install cmake"; fi
    confirm "Continue anyway (the build will fail until CMake is installed)?" n || die "install cmake and re-run."
  fi
  if [[ "$features" == *encode-mp3* ]]; then
    for t in autoconf automake libtool; do
      if ! have "$t" && ! { [[ "$t" == libtool ]] && have glibtool; }; then
        warn "encode-mp3 builds LAME with autotools; '$t' is missing."
        if [[ "$(uname -s)" == "Darwin" ]]; then info "  Install with: brew install autoconf automake libtool"
        else info "  Install with your package manager, e.g. apt install autoconf automake libtool"; fi
        confirm "Continue anyway?" n || die "install autotools and re-run."
        break
      fi
    done
  fi

  local scan="false"
  if (( ${#dirs[@]} > 0 )); then
    confirm "Scan the library every time the server starts?" y && scan="true"
  fi

  # -- write config
  step "6/6  Write config"
  local cfg="$ROOT/config.toml"
  ask "Config file" "$cfg"
  cfg="$REPLY"

  local dirs_toml="" first=1
  for d in ${dirs[@]+"${dirs[@]}"}; do
    [[ $first -eq 1 ]] || dirs_toml+=", "
    dirs_toml+="\"$(toml_str "$d")\""
    first=0
  done

  echo
  info "About to write ${B}$cfg${N}:"
  echo "${D}-----"
  cat <<EOF
music_dirs = [${dirs_toml}]
bind = "${bind}"
db_path = "$(toml_str "$db_path")"
preferred_ladder = [${ladder}]
dsd_story = "${dsd}"
scan_on_startup = ${scan}
EOF
  echo "-----${N}"
  confirm "Write it?" y || die "aborted; nothing written."

  backup_if_exists "$cfg"
  mkdir -p "$(dirname "$cfg")"
  cat > "$cfg" <<EOF
# Generated by scripts/setup.sh on $(date '+%Y-%m-%d %H:%M:%S')
# Trusted-LAN only: no authentication, no TLS.
music_dirs = [${dirs_toml}]
bind = "${bind}"
db_path = "$(toml_str "$db_path")"
preferred_ladder = [${ladder}]
dsd_story = "${dsd}"
scan_on_startup = ${scan}
EOF
  ok "wrote $cfg"

  SERVER_CONFIG="$cfg"
  SERVER_BIND="$bind"
  SERVER_PORT="$port"
  SERVER_FEATURES="$features"

  # -- build / run
  local feat_args=()
  [[ -n "$features" ]] && feat_args=(--features "$features")

  if confirm "Build the server now (release)?" y; then
    cargo build --release -p kahawai-server ${feat_args[@]+"${feat_args[@]}"}
    ok "built target/release/kahawai-server"
    if confirm "Start it now (Ctrl+C to stop)?" n; then
      info "Serving on http://${bind}  (config: $cfg)"
      exec "$ROOT/target/release/kahawai-server" "$cfg"
    fi
  fi

  echo
  info "Run the server any time with:"
  printf '  %scargo run --release -p kahawai-server%s%s -- %s\n' "$B" \
    "${features:+ --features $features}" "$N" "$cfg"
}

# ----------------------------------------------------------------- client ---
client_config_dir() {
  # Tauri's app_config_dir() for identifier com.suayan.kahawai-player.
  local id="com.suayan.kahawai-player"
  case "$(uname -s)" in
    Darwin) printf '%s' "$HOME/Library/Application Support/$id" ;;
    *)      printf '%s' "${XDG_CONFIG_HOME:-$HOME/.config}/$id" ;;
  esac
}

write_client_settings() {  # file url dsd global_format("" for auto)
  local file="$1" url="$2" dsd="$3" fmt="$4"
  if [[ -f "$file" ]] && have python3; then
    # Merge: keep the user's EQ/loudness settings, change only what we asked.
    python3 - "$file" "$url" "$dsd" "$fmt" <<'PY'
import json, sys
path, url, dsd, fmt = sys.argv[1:5]
try:
    with open(path) as f:
        cfg = json.load(f)
except Exception:
    cfg = {}
cfg["server_url"] = url
cfg["dsd_story"] = dsd
cfg["global_format"] = fmt or None
with open(path, "w") as f:
    json.dump(cfg, f, indent=2)
    f.write("\n")
PY
  else
    local gf="null"
    [[ -n "$fmt" ]] && gf="\"$fmt\""
    cat > "$file" <<EOF
{
  "server_url": "$url",
  "dsd_story": "$dsd",
  "global_format": $gf
}
EOF
  fi
}

setup_client() {
  step "Kahawai Player (desktop client)"

  # -- server URL
  step "1/4  Server address"
  local default_url="http://localhost:8080"
  if [[ -n "${SERVER_BIND:-}" ]]; then
    local host="${SERVER_BIND%%:*}"
    [[ "$host" == "0.0.0.0" ]] && host="localhost"
    default_url="http://${host}:${SERVER_PORT}"
    info "Using the server you just configured as the default."
  fi

  local url
  while true; do
    ask "Server URL" "$default_url"
    url="${REPLY%/}"
    [[ "$url" =~ ^https?:// ]] || url="http://$url"
    if have curl; then
      if health="$(curl -fsS --max-time 3 "$url/api/health" 2>/dev/null)"; then
        ok "server reachable: $health"
        break
      fi
      warn "no answer from $url/api/health"
      if confirm "Save it anyway (server not running yet / different machine)?" y; then break; fi
    else
      warn "curl not found; skipping the reachability check."
      break
    fi
  done

  # -- playback
  step "2/4  Playback"
  choose "DSD handling (client):" 1 \
    "convert - decode DSD as PCM (works with any output device)" \
    "native  - DoP to a DSD-capable DAC (macOS exclusive/hog mode)"
  local dsd="convert"
  [[ "$REPLY_INDEX" == "2" ]] && dsd="native"

  choose "Stream format override:" 1 \
    "auto (server ladder decides)   - recommended" \
    "flac" "opus" "mp3"
  local fmt=""
  case "$REPLY_INDEX" in
    2) fmt="flac" ;; 3) fmt="opus" ;; 4) fmt="mp3" ;;
  esac

  # -- write settings
  step "3/4  Save client settings"
  local dir file
  dir="$(client_config_dir)"
  file="$dir/engine-settings.json"
  info "Settings file: $file"
  if [[ -f "$file" ]] && ! have python3; then
    warn "python3 not found: the file will be replaced (EQ/loudness prefs reset)."
  fi
  confirm "Write it?" y || die "aborted; nothing written."
  mkdir -p "$dir"
  backup_if_exists "$file"
  write_client_settings "$file" "$url" "$dsd" "$fmt"
  ok "saved $file"
  info "${D}(You can change all of this later in the app's Settings screen.)${N}"

  # -- build prerequisites
  step "4/4  Build the app"
  if [[ ! -d "$ROOT/player/ui" ]]; then
    warn "player/ui not found; skipping."
    return
  fi
  if ! have node || ! have npm; then
    warn "Node.js/npm not found (needed for the Vue UI)."
    if [[ "$(uname -s)" == "Darwin" ]]; then info "  Install with: brew install node"; fi
    return
  fi
  if [[ ! -d "$ROOT/player/ui/node_modules" ]]; then
    if confirm "Install UI dependencies (npm install in player/ui)?" y; then
      (cd "$ROOT/player/ui" && npm install)
      ok "UI dependencies installed"
    fi
  else
    ok "UI dependencies already installed"
  fi

  if have cargo-tauri; then
    ok "tauri-cli present"
  else
    warn "tauri-cli is missing."
    if confirm "Install it now (cargo install tauri-cli; takes a few minutes)?" n; then
      cargo install tauri-cli
    else
      info "  Install later with: cargo install tauri-cli"
    fi
  fi

  echo
  info "Run the client in dev mode:"
  printf '  %s(cd player && cargo tauri dev)%s\n' "$B" "$N"
  if [[ "$(uname -s)" == "Darwin" ]]; then
    info "Or build the universal bundle:"
    printf '  %sscripts/build-client-universal.sh%s\n' "$B" "$N"
  fi
}

# ------------------------------------------------------------------- main ---
SERVER_CONFIG="" SERVER_BIND="" SERVER_PORT="" SERVER_FEATURES=""

mode="${1:-}"
case "$mode" in
  server|client|both) ;;
  ""|-h|--help)
    if [[ "$mode" != "" ]]; then
      sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
    fi
    printf '%s\nKahawai setup wizard%s\n' "$B" "$N"
    choose "What would you like to set up?" 3 \
      "Server only" \
      "Client only" \
      "Both (server first, then the client pointed at it)"
    case "$REPLY_INDEX" in 1) mode=server ;; 2) mode=client ;; 3) mode=both ;; esac
    ;;
  *) die "unknown argument '$mode' (use: server | client | both)" ;;
esac

[[ "$mode" == "server" || "$mode" == "both" ]] && setup_server
[[ "$mode" == "client" || "$mode" == "both" ]] && setup_client

step "Done"
ok "setup complete"
