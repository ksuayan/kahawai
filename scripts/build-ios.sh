#!/usr/bin/env bash
# Build the Kahawai Player as an iOS app (.ipa) for a device, or run it on the
# simulator in dev mode with scripts/start-dev-ios.sh.
#
#   scripts/build-ios.sh                  # debug build for a connected device
#   scripts/build-ios.sh --release        # release build (.ipa, needs signing)
#   scripts/build-ios.sh --simulator      # simulator build (no device needed)
#
# iOS background audio needs two things, both handled here:
#   1. UIBackgroundModes=audio in Info.plist (scripts/ios-plist-overlay.py).
#   2. The AVAudioSession in player/src-tauri/src/ios_audio.rs, driven from
#      the shell's player-state forwarding thread (Playback category, active
#      exactly while playing). There is no JS bridge and no service on iOS.
#
# Needs: macOS with Xcode (the iOS SDK), the Rust iOS target
# (`rustup target add aarch64-apple-ios aarch64-apple-ios-sim`), and tauri-cli.
# The iOS project (player/src-tauri/gen/ios) is generated on first use; it is
# gitignored, never commit it.
#
# Signing: a debug build installs on a device registered to your Apple ID
# (Xcode manages this; a free Apple ID works, provisioning expires in 7
# days). TestFlight / App Store distribution needs the paid Apple Developer
# Program membership. Until then, --simulator and free-ID device installs are
# the way to validate.
#
# NOTE: written alongside the Android scripts but not yet run — there is no
# Apple toolchain in this environment. Expect the first run on the Mac to
# surface small issues (target names, code-sign flags); fix them here.
set -euo pipefail

simulator=0
profile="debug"
for arg in "$@"; do
  case "$arg" in
    --simulator) simulator=1 ;;
    --release) profile="release" ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \\{0,1\\}//'; exit 0 ;;
    *) echo "error: unknown argument '$arg' (try --help)" >&2; exit 1 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

[[ "$(uname)" == "Darwin" ]] || { echo "error: iOS builds need macOS with Xcode." >&2; exit 1; }
command -v xcodebuild >/dev/null 2>&1 || { echo "error: Xcode not found." >&2; exit 1; }
command -v cargo-tauri >/dev/null 2>&1 || { echo "error: tauri-cli missing. Install: cargo install tauri-cli" >&2; exit 1; }

if [[ "$simulator" == 1 ]]; then
  triple="aarch64-apple-ios-sim"
else
  triple="aarch64-apple-ios"
fi
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
if [[ ! -d src-tauri/gen/ios ]]; then
  echo "==> Generating the iOS project (cargo tauri ios init)…"
  cargo tauri ios init --ci
fi

# Our own iOS additions over the generated project (gitignored): background
# audio mode. See player/src-tauri/ios-overlay.
python3 "$ROOT/scripts/ios-plist-overlay.py"

# WKWebView is modern Safari: no legacy-WebView flattening needed
# (KAHAWAI_LEGACY_WEBVIEW is for the HiBy R4's Chromium 91).

debug_flag=()
[[ "$profile" == "debug" ]] && debug_flag=(--debug)
echo "==> cargo tauri ios build ${debug_flag[*]} --target $triple"
cd "$ROOT/player/src-tauri"
cargo tauri ios build "${debug_flag[@]}" --target "$triple" --ci

echo
if [[ "$simulator" == 1 ]]; then
  echo "Simulator build done. Run it with: scripts/start-dev-ios.sh --simulator"
else
  echo "Device build done. Open the Xcode project to sign and install:"
  echo "  $ROOT/player/src-tauri/gen/ios"
fi
