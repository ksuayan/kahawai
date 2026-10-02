# Android build environment for the Kahawai Player: a JDK Gradle accepts,
# the SDK, the NDK, and the SDK tools on PATH. Sourced by build-android.sh
# and start-dev-android.sh; also usable in your own shell:
#
#   source scripts/android-env.sh
#
# Respects what is already set: ANDROID_HOME (default: the Homebrew
# android-commandlinetools location), NDK_HOME (default:
# $ANDROID_HOME/ndk/$NDK_VERSION), and JAVA_HOME when it is JDK 17 or newer
# (AGP 8+ needs 17+; otherwise the newest installed 21, then 17, is used).
# Not executable on its own; it only sets variables.

NDK_VERSION="${NDK_VERSION:-27.0.12077973}"
export ANDROID_HOME="${ANDROID_HOME:-/usr/local/share/android-commandlinetools}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export NDK_HOME="${NDK_HOME:-$ANDROID_HOME/ndk/$NDK_VERSION}"
export ANDROID_NDK_HOME="$NDK_HOME"
export PATH="$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$PATH"

# Major version of the JDK at $1 ("1.8.0_x" counts as 8).
_kahawai_java_major() {
  local v
  v="$("$1/bin/java" -version 2>&1 | awk -F'"' '/version/ { print $2; exit }')"
  [[ "$v" == 1.* ]] && v="${v#1.}"
  echo "${v%%.*}"
}

if [[ -z "${JAVA_HOME:-}" || ! -x "${JAVA_HOME}/bin/java" || "$(_kahawai_java_major "$JAVA_HOME")" -lt 17 ]]; then
  for _v in 21 17; do
    if _jh="$(/usr/libexec/java_home -v "$_v" 2>/dev/null)"; then
      export JAVA_HOME="$_jh"
      break
    fi
  done
  unset _v _jh
fi
