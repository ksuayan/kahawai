#!/usr/bin/env bash
# Build both macOS apps and package them into ONE combined .dmg, for the
# common case of running Player and Server on the same Mac.
#
#   scripts/build-combined-dmg.sh
#
# Runs build-client-universal.sh and build-server-app.sh (each already
# produces its own standalone dist/*.app + dist/*.dmg — this does not
# replace those, it adds a third option), then stages both .app bundles plus
# an /Applications symlink into a single disk image via hdiutil, the same
# drag-to-Applications convention either standalone .dmg already gives you.
#
# MAC-ONLY: producing universal binaries needs macOS (lipo and the Apple SDKs).
#
# Output:
#   dist/Kahawai Player.app       (from build-client-universal.sh)
#   dist/Kahawai Player.dmg
#   dist/Kahawai Server.app       (from build-server-app.sh)
#   dist/Kahawai Server.dmg
#   dist/Kahawai.dmg              (both apps together — this script's own output)
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: combined-dmg builds require macOS (this is $(uname -s))." >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

echo "==> building Kahawai Player…"
"${ROOT}/scripts/build-client-universal.sh"

echo "==> building Kahawai Server…"
"${ROOT}/scripts/build-server-app.sh"

OUT="${ROOT}/dist"
PLAYER_APP="${OUT}/Kahawai Player.app"
SERVER_APP="${OUT}/Kahawai Server.app"
COMBINED_DMG="${OUT}/Kahawai.dmg"

for app in "${PLAYER_APP}" "${SERVER_APP}"; do
  [[ -d "${app}" ]] || { echo "error: expected app bundle not found: ${app}" >&2; exit 1; }
done

echo "==> staging both apps for the combined image…"
STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT
cp -R "${PLAYER_APP}" "${STAGE}/"
cp -R "${SERVER_APP}" "${STAGE}/"
ln -s /Applications "${STAGE}/Applications"

echo "==> hdiutil create ${COMBINED_DMG}"
rm -f "${COMBINED_DMG}"
hdiutil create -volname "Kahawai" -srcfolder "${STAGE}" -ov -format UDZO "${COMBINED_DMG}"

echo
echo "Combined image ready (unsigned, unnotarized): ${COMBINED_DMG}"
echo "Opens to Kahawai Player.app, Kahawai Server.app, and an Applications shortcut."
echo "Sign/notarize each .app separately first (see build-client-universal.sh /"
echo "build-server-app.sh) if this DMG will be distributed outside your own Macs."
