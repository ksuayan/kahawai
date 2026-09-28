#!/usr/bin/env bash
# Build the macOS desktop shell (Kahawai Server) as a Universal app
# (x86_64 + arm64) into dist/.
#
#   scripts/build-server-app.sh
#
# `cargo tauri build --target universal-apple-darwin` builds the wizard UI
# (Vite) once, compiles the Rust binary for each architecture, and bundles
# the .app and a .dmg. kahawai-server is a workspace member (unlike the
# player's excluded src-tauri crate), so its build output lands in the
# shared workspace target/ rather than a crate-local one — see BUNDLES below.
#
# MAC-ONLY: producing a universal binary needs macOS (lipo and the Apple SDKs).
#
# Output:
#   dist/Kahawai Server.app
#   dist/Kahawai Server.dmg
#
# Signing & notarization are deliberately NOT done here; see the notes
# printed at the end (same manual steps as build-client-universal.sh).
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: universal server-app builds require macOS (this is $(uname -s))." >&2
  echo "On Linux, validate with: cargo check --workspace && cargo test --workspace" >&2
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
SERVER_CRATE="${ROOT}/crates/kahawai-server"

if [[ ! -d "${SERVER_CRATE}/ui/node_modules" ]]; then
  echo "error: crates/kahawai-server/ui/node_modules missing. Run 'npm install' there first." >&2
  echo "(build.rs will do this on its own during a plain 'cargo build', but 'cargo tauri build'" >&2
  echo " needs the beforeBuildCommand's npm to already be resolvable, i.e. installed once.)" >&2
  exit 1
fi

APP_NAME="Kahawai Server"  # must match productName in crates/kahawai-server/tauri.conf.json
EXE_NAME="kahawai-server"  # the Cargo bin name inside Contents/MacOS (not the product name)
# kahawai-server is a workspace member, so cargo uses the shared workspace
# target/ dir here — unlike the player's excluded src-tauri crate, which gets
# its own. (Confirmed empirically: a plain `cargo tauri build` from this
# directory bundles into ${ROOT}/target/release/bundle, not a crate-local one.)
BUNDLES="${ROOT}/target/universal-apple-darwin/release/bundle"

echo "==> cargo tauri build --target universal-apple-darwin (wizard UI once, Rust for both architectures)…"
cd "${SERVER_CRATE}"
cargo tauri build --target universal-apple-darwin

if [[ ! -d "${BUNDLES}/macos/${APP_NAME}.app" ]]; then
  echo "error: expected app bundle not found: ${BUNDLES}/macos/${APP_NAME}.app" >&2
  echo "Check that 'productName' in crates/kahawai-server/tauri.conf.json is '${APP_NAME}'." >&2
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

cat <<'EOF2'

Universal bundle is ready (unsigned, unnotarized).

Manual signing & notarization (requires a paid Apple Developer identity;
NOT attempted here):
  1. codesign --deep --force --options runtime \
       --sign "Developer ID Application: <Your Name> (<TEAMID>)" \
       "dist/Kahawai Server.app"
  2. ditto -c -k --keepParent "dist/Kahawai Server.app" "dist/Kahawai Server.zip"
  3. xcrun notarytool submit "dist/Kahawai Server.zip" \
       --apple-id <APPLE_ID> --team-id <TEAMID> --password <APP_SPECIFIC_PASSWORD> \
       --wait
  4. xcrun stapler staple "dist/Kahawai Server.app"
  5. spctl -a -vvv -t install "dist/Kahawai Server.app"   # sanity check
EOF2
