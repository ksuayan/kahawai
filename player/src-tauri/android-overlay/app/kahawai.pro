# Kept in player/src-tauri/android-overlay and copied over the generated
# project (player/src-tauri/gen/android, gitignored) by scripts/build-android.sh
# and scripts/start-dev-android.sh. The release build applies every *.pro
# under app/ (see app/build.gradle.kts), so this file is picked up
# automatically.

# The WebView JavascriptInterface bridges (MainActivity$InsetsBridge,
# MainActivity$PlaybackBridge) are invoked reflectively from JavaScript, which
# R8 cannot see: keep the annotated methods or release builds break the page
# -> native calls.
-keepclassmembers class com.suayan.kahawai.MainActivity$* {
  @android.webkit.JavascriptInterface <methods>;
}

# The foreground service behind background audio. It is referenced from the
# manifest (which AGP keeps automatically) and started via explicit Intent;
# this makes the keep explicit rather than relying on either path alone.
-keep class com.suayan.kahawai.PlaybackService { *; }
