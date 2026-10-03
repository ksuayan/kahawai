#!/usr/bin/env bash
# Run the player on the Android emulator in Tauri dev mode: the UI
# hot-reloads, Rust changes rebuild and reinstall the app.
#
#   scripts/start-dev-android.sh
#   scripts/start-dev-android.sh --avd kahawai35
#
# Steps, each skipped when already done:
#   1. Android environment (scripts/android-env.sh: JDK 17+, SDK, NDK).
#   2. Boot the emulator (AVD from --avd or KAHAWAI_AVD, default kahawai35)
#      and wait until Android has finished booting. A running emulator is
#      reused, and left running on exit (booting is slow); stop it with
#      `adb emu kill`.
#   3. npm dependencies for player/ui; stop a leftover Vite on :1420.
#   4. `cargo tauri android init` when player/src-tauri/gen/android is missing
#      (generated, gitignored; never commit it).
#   5. `cargo tauri android dev`: builds the Rust core for the emulator's ABI,
#      packages and installs the debug APK, launches it, and serves the UI from
#      Vite on :1420. Ctrl+C stops it.
#
# The server: inside the emulator `localhost` is the emulator itself. In the
# player's Settings use http://10.0.2.2:8080 (the emulator's alias for this
# Mac's localhost) with the server running on the Mac
# (scripts/start-server.sh).
#
# Create the AVD once with:
#   sdkmanager "emulator" "system-images;android-35;google_apis;x86_64"
#   avdmanager create avd -n kahawai35 -k "system-images;android-35;google_apis;x86_64"
#   sed -i '' 's/^hw.keyboard=no$/hw.keyboard=yes/' ~/.android/avd/kahawai35.avd/config.ini
# (avdmanager turns the Mac keyboard off by default; the sed turns it on.)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

avd="${KAHAWAI_AVD:-kahawai35}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --avd) avd="${2:?--avd needs an AVD name}"; shift 2 ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 1 ;;
  esac
done

# 1. Environment.
# shellcheck source=android-env.sh
source "$ROOT/scripts/android-env.sh"
command -v cargo-tauri >/dev/null 2>&1 || { echo "error: tauri-cli missing. Install: cargo install tauri-cli" >&2; exit 1; }
for tool in adb emulator; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "error: '$tool' not found under ANDROID_HOME=$ANDROID_HOME." >&2
    echo "Install with: sdkmanager \"platform-tools\" \"emulator\"" >&2
    exit 1
  }
done
[[ -d "$NDK_HOME" ]] || { echo "error: Android NDK not found at $NDK_HOME (sdkmanager \"ndk;$NDK_VERSION\")" >&2; exit 1; }
[[ -n "${JAVA_HOME:-}" ]] || { echo "error: no JDK 17+ found; Gradle needs one." >&2; exit 1; }
echo "==> JDK: $JAVA_HOME"

# 2. Emulator.
running_emulator() { adb devices | awk '$1 ~ /^emulator-/ && $2 == "device" { print $1; exit }'; }

serial="$(running_emulator)"
if [[ -z "$serial" ]]; then
  if ! emulator -list-avds | grep -qx "$avd"; then
    echo "error: no AVD named '$avd'. Available:" >&2
    emulator -list-avds | sed 's/^/  /' >&2
    echo "Create one (see --help) or pass --avd <name>." >&2
    exit 1
  fi
  log="${TMPDIR:-/tmp}/kahawai-emulator-$avd.log"
  echo "==> Booting emulator '$avd' (log: $log)…"
  nohup emulator -avd "$avd" -netdelay none -netspeed full >"$log" 2>&1 &
  adb wait-for-device
  serial="$(running_emulator)"
fi

echo "==> Waiting for Android to finish booting on ${serial:-the emulator}…"
for _ in $(seq 1 180); do
  [[ "$(adb -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == "1" ]] && break
  sleep 1
  serial="${serial:-$(running_emulator)}"
done
[[ "$(adb -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == "1" ]] || {
  echo "error: the emulator did not finish booting within 3 minutes." >&2
  exit 1
}

# The Rust target matching the emulator's ABI.
abi="$(adb -s "$serial" shell getprop ro.product.cpu.abi | tr -d '\r')"
case "$abi" in
  x86_64)      triple="x86_64-linux-android" ;;
  arm64-v8a)   triple="aarch64-linux-android" ;;
  *) echo "error: unsupported emulator ABI '$abi'." >&2; exit 1 ;;
esac
if ! rustup target list --installed | grep -q "^${triple}$"; then
  echo "error: Rust target '${triple}' is not installed (emulator ABI $abi)." >&2
  echo "Install with: rustup target add ${triple}" >&2
  exit 1
fi
echo "==> Emulator $serial ready ($abi)"

# 3. Frontend dependencies and a free Vite port.
stamp="$ROOT/player/ui/node_modules/.package-lock.json"
if [[ ! -f "$stamp" || "$ROOT/player/ui/package.json" -nt "$stamp" || "$ROOT/player/ui/package-lock.json" -nt "$stamp" ]]; then
  echo "==> npm install in player/ui (dependencies missing or changed)…"
  (cd "$ROOT/player/ui" && npm install --no-audit --no-fund)
  touch "$stamp"
fi

for pid in $(lsof -nP -iTCP:1420 -sTCP:LISTEN -t 2>/dev/null | sort -u); do
  cmd="$(ps -o command= -p "$pid" 2>/dev/null || true)"
  if [[ "$cmd" == *vite* ]]; then
    echo "Stopping a leftover Vite dev server on :1420 (pid $pid)"
    kill "$pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 0.3; done
  else
    echo "error: port 1420 is in use by something other than Vite: $cmd" >&2
    exit 1
  fi
done

# 4. The generated Android project.
cd "$ROOT/player"
if [[ ! -d src-tauri/gen/android ]]; then
  echo "==> Generating the Android project (cargo tauri android init)…"
  cargo tauri android init --ci
fi

# Our own Android sources over the generated project (gitignored): see
# player/src-tauri/android-overlay (the MainActivity with its JS bridges,
# the PlaybackService for background audio).
cp -R "$ROOT/player/src-tauri/android-overlay/." "$ROOT/player/src-tauri/gen/android/"

# Background-audio manifest entries (foreground-service permissions and the
# PlaybackService declaration) merged into the generated manifest.
python3 "$ROOT/scripts/android-manifest-overlay.py"

# Old device WebViews ignore Tailwind 4's cascade layers; flatten them (vite.config.ts).
export KAHAWAI_LEGACY_WEBVIEW=1

# 5. Build, install, launch, hot-reload.
echo "==> cargo tauri android dev (Ctrl+C to stop; the emulator keeps running)"
exec cargo tauri android dev
