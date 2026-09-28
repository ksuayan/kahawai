#!/usr/bin/env bash
# Reset build state before a fresh build: stop running app/server instances,
# unmount any leftover DMG staging volumes, and remove build outputs.
#
#   scripts/cleanup.sh              # stop everything + remove all build output
#   scripts/cleanup.sh --quick      # same, but keeps target/ (skips the full
#                                    # recompile a `cargo clean`-equivalent forces)
#   scripts/cleanup.sh --dry-run    # print what would happen, change nothing
#
# Never touches: config.toml, config.toml.bak.*, data/ (your library + DB),
# .kahawai-server.pid / kahawai-server.log while a server is still running
# (stopped cleanly via start-server.sh --stop first, not deleted out from
# under it), or any node_modules (not a build-output problem, and reinstalling
# them is the expensive part of "fresh").
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"

quick=0 dry_run=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --quick)   quick=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 1 ;;
  esac
done

run() {
  if [[ "${dry_run}" -eq 1 ]]; then
    echo "would run: $*"
  else
    echo "==> $*"
    "$@"
  fi
}

rm_path() {
  local path="$1"
  if [[ -e "${path}" ]]; then
    run rm -rf "${path}"
  fi
}

echo "== stopping running instances =="

# The headless server, if backgrounded via start-server.sh -d: this respects
# its pid file and stops it gracefully (SIGTERM, then SIGKILL after a grace
# period) rather than a bare kill.
if [[ -f "${ROOT}/.kahawai-server.pid" ]]; then
  run "${ROOT}/scripts/start-server.sh" --stop
fi

# Any foreground/debug-build/.app instances of either app, headless or GUI.
# -f matches the full command line, so this catches target/debug,
# target/release, dist/, and *.app/Contents/MacOS/ binaries alike.
for pattern in "MacOS/kahawai-server" "MacOS/kahawai-player" \
               "target/debug/kahawai-server" "target/release/kahawai-server" \
               "dist/kahawai-server" "target/debug/kahawai-player" "target/release/kahawai-player"; do
  if pgrep -f "${pattern}" >/dev/null 2>&1; then
    if [[ "${dry_run}" -eq 1 ]]; then
      echo "would run: pkill -f '${pattern}'"
    else
      echo "==> pkill -f '${pattern}'"
      pkill -f "${pattern}" 2>/dev/null || true
    fi
  fi
done

echo "== unmounting leftover DMG volumes =="

# bundle_dmg.sh (used by `cargo tauri build`'s dmg target) mounts a
# read/write staging volume (same volname as the final .dmg) while it works;
# if a build was interrupted, that can be left mounted and will block
# deleting the target/ directories below.
for vol in /Volumes/Kahawai*; do
  [[ -d "${vol}" ]] || continue
  run hdiutil detach "${vol}" -quiet -force
done

echo "== removing build output =="

rm_path "${ROOT}/dist"
rm_path "${ROOT}/crates/kahawai-server/ui/dist"
rm_path "${ROOT}/crates/kahawai-server/gen"
rm_path "${ROOT}/player/ui/dist"
rm_path "${ROOT}/player/src-tauri/gen"

if [[ "${quick}" -eq 0 ]]; then
  rm_path "${ROOT}/target"
  rm_path "${ROOT}/player/src-tauri/target"
else
  echo "(--quick: leaving target/ and player/src-tauri/target/ in place)"
fi

echo
echo "Clean. Untouched: config.toml, data/, node_modules (player/ui, crates/kahawai-server/ui)."
[[ "${dry_run}" -eq 1 ]] && echo "(--dry-run: nothing was actually changed)"
