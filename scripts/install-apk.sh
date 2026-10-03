#!/usr/bin/env bash
# Install the Kahawai Player APK on a USB-connected Android device (a HiBy R4,
# or any arm64 phone or player) and start it.
#
#   scripts/install-apk.sh                    # the newest debug APK for the device's ABI
#   scripts/install-apk.sh path/to/app.apk    # a particular APK
#   scripts/install-apk.sh --serial <id>      # with several devices connected
#   scripts/install-apk.sh --reinstall        # uninstall first (see below)
#   scripts/install-apk.sh --no-launch        # install only
#
# Build the APK first with scripts/build-android.sh (arm64 debug by default).
#
# On the device, once: Settings > About device, tap "Build number" seven times
# to unlock Developer options, then turn on USB debugging there. Connect the
# USB cable and accept the "Allow USB debugging?" prompt on the device.
#
# An install over a copy signed with a different key (a build from another
# Mac, or a release build over a debug one) is refused by Android.
# --reinstall uninstalls the old copy first, which also erases the app's data
# on the device (its settings, such as the server address).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

apk=""
serial="${ANDROID_SERIAL:-}"
reinstall=0
launch=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --serial) serial="${2:?--serial needs a device id (see: adb devices)}"; shift 2 ;;
    --reinstall) reinstall=1; shift ;;
    --no-launch) launch=0; shift ;;
    -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "error: unknown option '$1' (see --help)" >&2; exit 1 ;;
    *) apk="$1"; shift ;;
  esac
done

# adb from the Android SDK.
# shellcheck source=android-env.sh
source "$ROOT/scripts/android-env.sh"
command -v adb >/dev/null 2>&1 || {
  echo "error: adb not found under ANDROID_HOME=$ANDROID_HOME. Install with: sdkmanager \"platform-tools\"" >&2
  exit 1
}

# The device: the one given, or the only connected one (a real device is
# preferred over a running emulator).
devices="$(adb devices | awk 'NR > 1 && NF >= 2 { print $1 "\t" $2 }')"
if [[ -z "$serial" ]]; then
  ready="$(printf '%s\n' "$devices" | awk -F'\t' '$2 == "device" { print $1 }')"
  real="$(printf '%s\n' "$ready" | grep -v '^emulator-' || true)"
  pick="${real:-$ready}"
  count="$(printf '%s\n' "$pick" | grep -c . || true)"
  if [[ "$count" -eq 0 ]]; then
    if printf '%s\n' "$devices" | grep -q $'\tunauthorized'; then
      echo "error: the device has not allowed USB debugging yet: accept the prompt on its screen, then run this again." >&2
    else
      echo "error: no Android device found. Connect it by USB with USB debugging on (see --help)." >&2
    fi
    exit 1
  fi
  if [[ "$count" -gt 1 ]]; then
    echo "error: several devices are connected; choose one with --serial:" >&2
    printf '%s\n' "$pick" | sed 's/^/  /' >&2
    exit 1
  fi
  serial="$pick"
fi
adb_s() { adb -s "$serial" "$@"; }
state="$(adb_s get-state 2>/dev/null || true)"
[[ "$state" == "device" ]] || { echo "error: device $serial is not ready (state: ${state:-not connected})." >&2; exit 1; }

model="$(adb_s shell getprop ro.product.model | tr -d '\r')"
abi="$(adb_s shell getprop ro.product.cpu.abi | tr -d '\r')"
echo "==> Device: ${model:-unknown} ($serial, $abi)"

# The APK: the one given, or the newest debug build for the device's ABI.
outputs="$ROOT/player/src-tauri/gen/android/app/build/outputs/apk"
if [[ -z "$apk" ]]; then
  case "$abi" in
    arm64-v8a)   abi_dir="arm64" ;;
    armeabi-v7a) abi_dir="arm" ;;
    x86_64)      abi_dir="x86_64" ;;
    x86)         abi_dir="x86" ;;
    *) echo "error: unsupported device ABI '$abi'." >&2; exit 1 ;;
  esac
  apk="$(ls -t "$outputs/$abi_dir/debug/"*.apk "$outputs/universal/debug/"*.apk 2>/dev/null | head -n 1 || true)"
  if [[ -z "$apk" ]]; then
    echo "error: no debug APK for $abi under $outputs." >&2
    echo "Build one with: scripts/build-android.sh $( [[ "$abi_dir" == "arm64" ]] && echo aarch64 || echo "$abi_dir" )" >&2
    exit 1
  fi
fi
[[ -f "$apk" ]] || { echo "error: no such file: $apk" >&2; exit 1; }
if [[ "$apk" == *-unsigned.apk ]]; then
  echo "error: $apk is unsigned; Android will not install it. Use a debug build, or sign it first." >&2
  exit 1
fi

# The app's id, from the generated Android project (it is not the Tauri identifier).
gradle="$ROOT/player/src-tauri/gen/android/app/build.gradle.kts"
package="$(sed -n 's/^[[:space:]]*applicationId = "\(.*\)"/\1/p' "$gradle" 2>/dev/null | head -n 1)"
package="${package:-com.suayan.kahawai}"

if [[ "$reinstall" -eq 1 ]] && adb_s shell pm list packages "$package" | tr -d '\r' | grep -qx "package:$package"; then
  echo "==> Uninstalling the old copy of $package (its data on the device goes with it)…"
  adb_s uninstall "$package" >/dev/null
fi

echo "==> Installing $(basename "$apk") ($(du -h "$apk" | cut -f1))…"
if ! out="$(adb_s install -r "$apk" 2>&1)"; then
  echo "$out" >&2
  if [[ "$out" == *INSTALL_FAILED_UPDATE_INCOMPATIBLE* || "$out" == *signatures*do*not*match* ]]; then
    echo "error: the copy on the device is signed with a different key. Run again with --reinstall (this erases the app's data on the device)." >&2
  elif [[ "$out" == *INSTALL_FAILED_NO_MATCHING_ABIS* ]]; then
    echo "error: this APK is not built for the device's $abi processor." >&2
  fi
  exit 1
fi
echo "$out" | tail -n 1

if [[ "$launch" -eq 1 ]]; then
  echo "==> Starting Kahawai Player…"
  adb_s shell monkey -p "$package" -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1 \
    || echo "note: installed, but it could not be started from here; open it on the device." >&2
fi
echo "Done. In the player's Settings, set the server to the address of the Mac running Kahawai Server, e.g. http://192.168.1.20:8080."
