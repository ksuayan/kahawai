#!/usr/bin/env bash
# Build a macOS Universal Binary of the desktop client (Tauri app) via
# per-arch `cargo tauri build` + lipo of the resulting app bundle's binary.
#
# MAC-ONLY: this script refuses to run on any other OS. Producing a universal
# binary requires macOS (for lipo and the Apple SDKs). On Linux, validate the
# client code with `cargo check` / `cargo test` in kahawai-player-core and the npm
# gates in player/ui instead.
#
# UNTESTED: the CoreAudio DoP path has never been compiled or run in this
# workspace (Linux has no Apple SDK/libclang). The first real validation
# happens on a Mac: `cargo tauri build` for one arch, play to a real DAC,
# verify hog mode, sample-rate switching, and 24-bit packed output, then
# run this script for the universal bundle.
#
# Signing & notarization are deliberately NOT done here. They require a
# paid Apple Developer identity and manual steps — see the bottom of the
# script output for the manual recipe.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: universal client builds require macOS (this is $(uname -s))." >&2
  echo "On Linux, validate with: cd player && cargo check (needs GTK/WebKit dev packages)" >&2
  echo "and the player/ui npm gates (typecheck + build)." >&2
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

echo "==> building frontend (player/ui)…"
cd "${PLAYER}/ui"
if [[ -d node_modules ]]; then
  npm run build
else
  echo "error: player/ui/node_modules missing. Run 'npm install' in player/ui first." >&2
  exit 1
fi

APP_NAME="Kahawai Player"  # must match productName in player/src-tauri/tauri.conf.json
EXE_NAME="kahawai-player"  # the Cargo bin name inside Contents/MacOS (not the product name)

for target in x86_64-apple-darwin aarch64-apple-darwin; do
  echo "==> cargo tauri build --target ${target}…"
  cd "${PLAYER}"
  cargo tauri build --target "${target}"
done

BUNDLE_X64="${PLAYER}/src-tauri/target/x86_64-apple-darwin/release/bundle/macos"
BUNDLE_ARM="${PLAYER}/src-tauri/target/aarch64-apple-darwin/release/bundle/macos"

if [[ ! -d "${BUNDLE_X64}/${APP_NAME}.app" || ! -d "${BUNDLE_ARM}/${APP_NAME}.app" ]]; then
  echo "error: expected app bundles not found under:" >&2
  echo "  ${BUNDLE_X64}" >&2
  echo "  ${BUNDLE_ARM}" >&2
  echo "Check that 'productName' in player/src-tauri/tauri.conf.json is '${APP_NAME}'." >&2
  exit 1
fi

OUT="${ROOT}/dist/${APP_NAME}.app"
echo "==> assembling universal bundle → ${OUT}"
rm -rf "${OUT}"
cp -R "${BUNDLE_ARM}/${APP_NAME}.app" "${OUT}"

BIN_X64="${BUNDLE_X64}/${APP_NAME}.app/Contents/MacOS/${EXE_NAME}"
BIN_ARM="${BUNDLE_ARM}/${APP_NAME}.app/Contents/MacOS/${EXE_NAME}"
BIN_OUT="${OUT}/Contents/MacOS/${EXE_NAME}"

if [[ ! -f "${BIN_X64}" || ! -f "${BIN_ARM}" ]]; then
  echo "error: app bundle executables not found." >&2
  exit 1
fi

lipo -create "${BIN_X64}" "${BIN_ARM}" -output "${BIN_OUT}"

echo "==> verifying"
lipo -archs "${BIN_OUT}"
file "${BIN_OUT}"

cat <<'EOF'

Universal bundle is ready (unsigned, unnotarized).

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
EOF
