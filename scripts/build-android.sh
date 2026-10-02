#!/usr/bin/env bash
# Build the Kahawai Player Rust core for Android with plain cargo
# (libkahawai_player.so, the cdylib the Android app loads through JNI).
#
#   scripts/build-android.sh                  # aarch64 (phones), debug
#   scripts/build-android.sh x86_64           # emulator on an Intel Mac
#   scripts/build-android.sh aarch64 --release
#
# Targets: aarch64 (default), armv7, x86_64, i686.
#
# `cargo tauri android build` / `dev` point cargo at the NDK compilers
# themselves; plain `cargo build --target <android>` does not, and C/C++ build
# scripts (ring, opusic-sys) then fail looking for e.g.
# `aarch64-linux-android-clang`. This script sets CC/CXX/AR and the linker for
# the chosen target from the NDK, then builds the lib.
#
# Needs: the Android NDK (located by android-env.sh: NDK_HOME, or
# ANDROID_HOME/ndk/<NDK_VERSION>) and the Rust target (`rustup target add
# <triple>`). No JDK or Gradle — that is the APK step, run from
# player/src-tauri/gen/android after `cargo tauri android init`.
#
# Output:
#   player/src-tauri/target/<triple>/{debug,release}/libkahawai_player.so
set -euo pipefail

# The API level baked into the NDK clang wrappers. Matches
# bundle.android.minSdkVersion in tauri.conf.json (AAudio floor for CPAL/Oboe).
API_LEVEL=26

arch="aarch64"
profile_flag=""
for arg in "$@"; do
  case "$arg" in
    --release) profile_flag="--release" ;;
    aarch64|armv7|x86_64|i686) arch="$arg" ;;
    -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument '$arg' (try --help)" >&2; exit 1 ;;
  esac
done

case "$arch" in
  aarch64) triple="aarch64-linux-android";   clang_prefix="aarch64-linux-android" ;;
  armv7)   triple="armv7-linux-androideabi"; clang_prefix="armv7a-linux-androideabi" ;;
  x86_64)  triple="x86_64-linux-android";    clang_prefix="x86_64-linux-android" ;;
  i686)    triple="i686-linux-android";      clang_prefix="i686-linux-android" ;;
esac

# ANDROID_HOME, NDK_HOME, NDK_VERSION (and JAVA_HOME, unused here).
# shellcheck source=android-env.sh
source "$(dirname "$0")/android-env.sh"
if [[ ! -d "$NDK_HOME" ]]; then
  echo "error: Android NDK not found at $NDK_HOME" >&2
  echo "Install with: sdkmanager \"ndk;$NDK_VERSION\" (or set NDK_HOME)" >&2
  exit 1
fi

# The NDK ships one host toolchain per OS; on macOS it is named darwin-x86_64
# but is universal (runs natively on Apple Silicon too).
case "$(uname -s)" in
  Darwin) host_tag="darwin-x86_64" ;;
  Linux)  host_tag="linux-x86_64" ;;
  *) echo "error: unsupported host $(uname -s)" >&2; exit 1 ;;
esac
NDK_BIN="$NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin"
clang="$NDK_BIN/${clang_prefix}${API_LEVEL}-clang"
if [[ ! -x "$clang" ]]; then
  echo "error: NDK compiler not found: $clang" >&2
  exit 1
fi

if ! rustup target list --installed | grep -q "^${triple}$"; then
  echo "error: Rust target '${triple}' is not installed." >&2
  echo "Install with: rustup target add ${triple}" >&2
  exit 1
fi

# cc-rs reads CC_<triple> etc. with the triple's dashes as underscores; cargo
# reads CARGO_TARGET_<TRIPLE>_LINKER uppercased.
triple_us="${triple//-/_}"
triple_uc="$(echo "$triple_us" | tr '[:lower:]' '[:upper:]')"
export "CC_${triple_us}=$clang"
export "CXX_${triple_us}=${clang}++"
export "AR_${triple_us}=$NDK_BIN/llvm-ar"
export "CARGO_TARGET_${triple_uc}_LINKER=$clang"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root/player/src-tauri"

echo "==> cargo build --target $triple --lib ${profile_flag} (NDK $(basename "$NDK_HOME"), API $API_LEVEL)"
cargo build --target "$triple" --lib ${profile_flag}

profile_dir="debug"
[[ -n "$profile_flag" ]] && profile_dir="release"
so="$PWD/target/$triple/$profile_dir/libkahawai_player.so"
echo
echo "Built: $so ($(du -h "$so" | cut -f1))"
