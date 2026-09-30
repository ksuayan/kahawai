#!/usr/bin/env bash
# Build the macOS desktop client (Kahawai Player) as a Universal app
# (x86_64 + arm64) into dist/.
#
#   scripts/build-client-universal.sh
#
# One command does everything, once: `cargo tauri build --target
# universal-apple-darwin` runs the frontend build (Vite) a single time,
# compiles the Rust shell for each architecture (that is what "universal"
# means: two compiles, merged into one binary), and bundles the .app and a
# .dmg. The frontend is identical for both architectures, so it is built once,
# not once per architecture.
#
# MAC-ONLY: producing a universal binary needs macOS (lipo and the Apple SDKs).
#
# Output:
#   dist/Kahawai Player.app
#   dist/Kahawai Player.dmg
#
# Signing & notarization are deliberately NOT done here. They require a paid
# Apple Developer identity and manual steps; see the notes printed at the end.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: universal client builds require macOS (this is $(uname -s))." >&2
  echo "On Linux, validate with the player/ui npm gates (npm test, npm run build)." >&2
  exit 1
fi

for target in x86_64-apple-darwin aarch64-apple-darwin; do
  if ! rustup target list --installed | grep -q "^${target}$"; then
    echo "error: Rust target '${target}' is not installed." >&2
    echo "Install with: rustup target add ${target}" >&2
    exit 1
  fi
done

command -v cargo-tauri >/dev/null 2>&1 || {
  echo "error: tauri-cli is not installed. Install with: cargo install tauri-cli" >&2
  exit 1
}

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PLAYER="${ROOT}/player"

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

ensure_npm_deps "${PLAYER}/ui"

APP_NAME="Kahawai Player"  # must match productName in player/src-tauri/tauri.conf.json
EXE_NAME="kahawai-player"  # the Cargo bin name inside Contents/MacOS (not the product name)
BUNDLES="${PLAYER}/src-tauri/target/universal-apple-darwin/release/bundle"

echo "==> cargo tauri build --target universal-apple-darwin (frontend once, Rust for both architectures)…"
# Must cd into src-tauri itself (where tauri.conf.json lives), not its parent:
# run from player/, the CLI's beforeBuildCommand resolves relative to the
# frontendDist's directory instead of src-tauri's, doubling "ui" (it looks
# for player/ui/ui/package.json instead of player/ui/package.json).
cd "${PLAYER}/src-tauri"
cargo tauri build --target universal-apple-darwin

if [[ ! -d "${BUNDLES}/macos/${APP_NAME}.app" ]]; then
  echo "error: expected app bundle not found: ${BUNDLES}/macos/${APP_NAME}.app" >&2
  echo "Check that 'productName' in player/src-tauri/tauri.conf.json is '${APP_NAME}'." >&2
  exit 1
fi

OUT="${ROOT}/dist"
mkdir -p "${OUT}"
echo "==> copying to ${OUT}"
rm -rf "${OUT}/${APP_NAME}.app"
cp -R "${BUNDLES}/macos/${APP_NAME}.app" "${OUT}/${APP_NAME}.app"
dmg="$(ls "${BUNDLES}"/dmg/*.dmg 2>/dev/null | head -n 1 || true)"
if [[ -n "${dmg}" ]]; then
  cp -f "${dmg}" "${OUT}/${APP_NAME}.dmg"
fi

echo "==> verifying"
BIN_OUT="${OUT}/${APP_NAME}.app/Contents/MacOS/${EXE_NAME}"
archs="$(lipo -archs "${BIN_OUT}")"
echo "architectures: ${archs}"
case "${archs}" in
  *x86_64*arm64*|*arm64*x86_64*) ;;
  *) echo "error: ${BIN_OUT} is not universal (${archs})." >&2; exit 1 ;;
esac
file "${BIN_OUT}"
# The bundle must carry a valid (ad-hoc) signature: on Apple Silicon a
# downloaded app with an unsigned bundle is refused as "damaged". Tauri signs
# it (bundle.macOS.signingIdentity "-" in tauri.conf.json).
if ! codesign --verify --deep --strict "${OUT}/${APP_NAME}.app"; then
  echo "error: ${APP_NAME}.app is not properly signed (see signingIdentity in tauri.conf.json)." >&2
  exit 1
fi
codesign -dv "${OUT}/${APP_NAME}.app" 2>&1 | grep -E "^Signature=" || true

cat <<'EOF2'

Universal bundle is ready (ad-hoc signed, not notarized).

Manual signing & notarization (requires a paid Apple Developer identity;
NOT attempted here):
  1. codesign --deep --force --options runtime \
       --sign "Developer ID Application: <Your Name> (<TEAMID>)" \
       "dist/Kahawai Player.app"
  2. ditto -c -k --keepParent "dist/Kahawai Player.app" "dist/Kahawai Player.zip"
  3. xcrun notarytool submit "dist/Kahawai Player.zip" \
       --apple-id <APPLE_ID> --team-id <TEAMID> --password <APP_SPECIFIC_PASSWORD> \
       --wait
  4. xcrun stapler staple "dist/Kahawai Player.app"
  5. spctl -a -vvv -t install "dist/Kahawai Player.app"   # sanity check
EOF2
