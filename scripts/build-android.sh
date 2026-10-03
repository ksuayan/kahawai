#!/usr/bin/env bash
# Build the Kahawai Player as an Android app (.apk) to install on a device:
# a HiBy R4 or any other arm64 phone or player.
#
#   scripts/build-android.sh                  # arm64 (devices), debug APK
#   scripts/build-android.sh x86_64           # the emulator on an Intel Mac
#   scripts/build-android.sh aarch64 --release
#
# Targets: aarch64 (default), armv7, x86_64, i686.
#
# Debug (the default) is what you want for a device: it is signed with the
# debug key, so it installs straight away (scripts/install-apk.sh), and it may
# talk to the server over plain http:// on your network. A release build is
# unsigned (sign it with your own key before Android will install it) and, as
# generated, blocks plain http://, so it cannot reach a LAN server until the
# Android project allows cleartext traffic.
#
# `cargo tauri android build` does the whole job: it builds the UI
# (build.beforeBuildCommand), cross-compiles the Rust core with the NDK's
# compilers, and packages the APK with Gradle. Unlike `android dev`, the UI is
# inside the APK, so the app works without this Mac's dev server.
#
# Needs: a JDK 17+, the Android SDK and NDK (located by android-env.sh), the
# Rust target (`rustup target add <triple>`), and tauri-cli. The Android
# project (player/src-tauri/gen/android) is generated on first use; it is
# gitignored, never commit it.
#
# Output (printed at the end):
#   player/src-tauri/gen/android/app/build/outputs/apk/<abi>/{debug,release}/app-<abi>-*.apk
set -euo pipefail

arch="aarch64"
profile="debug"
for arg in "$@"; do
  case "$arg" in
    --release) profile="release" ;;
    aarch64|armv7|x86_64|i686) arch="$arg" ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument '$arg' (try --help)" >&2; exit 1 ;;
  esac
done

case "$arch" in
  aarch64) triple="aarch64-linux-android";   abi_dir="arm64" ;;
  armv7)   triple="armv7-linux-androideabi"; abi_dir="arm" ;;
  x86_64)  triple="x86_64-linux-android";    abi_dir="x86_64" ;;
  i686)    triple="i686-linux-android";      abi_dir="x86" ;;
esac

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# ANDROID_HOME, NDK_HOME, NDK_VERSION, JAVA_HOME.
# shellcheck source=android-env.sh
source "$ROOT/scripts/android-env.sh"
command -v cargo-tauri >/dev/null 2>&1 || { echo "error: tauri-cli missing. Install: cargo install tauri-cli" >&2; exit 1; }
[[ -d "$NDK_HOME" ]] || {
  echo "error: Android NDK not found at $NDK_HOME" >&2
  echo "Install with: sdkmanager \"ndk;$NDK_VERSION\" (or set NDK_HOME)" >&2
  exit 1
}
[[ -n "${JAVA_HOME:-}" && -x "$JAVA_HOME/bin/java" ]] || { echo "error: no JDK 17+ found; Gradle needs one." >&2; exit 1; }
if ! rustup target list --installed | grep -q "^${triple}$"; then
  echo "error: Rust target '${triple}' is not installed." >&2
  echo "Install with: rustup target add ${triple}" >&2
  exit 1
fi

# The UI's npm dependencies, installed when missing or out of date.
stamp="$ROOT/player/ui/node_modules/.package-lock.json"
if [[ ! -f "$stamp" || "$ROOT/player/ui/package.json" -nt "$stamp" || "$ROOT/player/ui/package-lock.json" -nt "$stamp" ]]; then
  echo "==> npm install in player/ui (dependencies missing or changed)…"
  (cd "$ROOT/player/ui" && npm install --no-audit --no-fund)
  touch "$stamp"
fi

cd "$ROOT/player"
if [[ ! -d src-tauri/gen/android ]]; then
  echo "==> Generating the Android project (cargo tauri android init)…"
  cargo tauri android init --ci
fi

# Our own Android sources over the generated project (gitignored): see
# player/src-tauri/android-overlay (the MainActivity that passes the system
# bars' size to the page).
cp -R "$ROOT/player/src-tauri/android-overlay/." "$ROOT/player/src-tauri/gen/android/"

# Android devices can ship an old system WebView (the HiBy R4: Chromium 91)
# that ignores Tailwind 4's cascade layers; this flattens them (vite.config.ts).
export KAHAWAI_LEGACY_WEBVIEW=1

debug_flag=()
[[ "$profile" == "debug" ]] && debug_flag=(--debug)
echo "==> cargo tauri android build ${debug_flag[*]} --target $arch --apk --split-per-abi (JDK $JAVA_HOME, NDK $(basename "$NDK_HOME"))"
# A fresh APK each time: Gradle's incremental packaging updates the old file in
# place and leaves the space the large native library used to take, so the APK
# grows (here from about 250 MB to nearly 500 MB) without holding more.
rm -f "$ROOT/player/src-tauri/gen/android/app/build/outputs/apk/$abi_dir/$profile/"*.apk
started="$(date +%s)"
# Run from src-tauri itself (where tauri.conf.json lives), as the desktop build
# does: from player/, the CLI runs beforeBuildCommand from player/ui and looks
# for player/ui/ui/package.json.
cd "$ROOT/player/src-tauri"
cargo tauri android build "${debug_flag[@]}" --target "$arch" --apk --split-per-abi --ci

# The APK this run produced (the newest under this ABI and profile).
out="$ROOT/player/src-tauri/gen/android/app/build/outputs/apk/$abi_dir/$profile"
apk="$(ls -t "$out"/*.apk 2>/dev/null | head -n 1 || true)"
if [[ -z "$apk" || "$(stat -f %m "$apk" 2>/dev/null || stat -c %Y "$apk")" -lt "$started" ]]; then
  echo "error: no new APK found in $out" >&2
  exit 1
fi
echo
echo "Built: $apk ($(du -h "$apk" | cut -f1))"
if [[ "$profile" == "release" ]]; then
  echo "note: a release APK is unsigned and blocks plain http:// as generated; sign it before installing." >&2
else
  echo "Install it with: scripts/install-apk.sh"
fi
