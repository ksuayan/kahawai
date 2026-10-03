# iOS overlay

Kept in `player/src-tauri/ios-overlay` and copied over the generated iOS
project (`player/src-tauri/gen/ios`, gitignored) by `scripts/build-ios.sh`
and `scripts/start-dev-ios.sh`. Edit this copy, not the generated one.

## What lives here (planned)

| File | Purpose |
|---|---|
| `Info.plist` additions (applied by `scripts/ios-plist-overlay.py`) | `UIBackgroundModes: [audio]` — without this, iOS suspends audio when the app backgrounds or the screen locks. The Rust side (`src/ios_audio.rs`) activates the matching `AVAudioSession`. |

## Deliberately empty for now

- **No Swift AppDelegate replacement.** Background audio needs no app-delegate
  code: the plist entry plus the Rust-driven `AVAudioSession` (via
  `objc2-avf-audio`) is the whole story. If later work needs native hooks
  (lock-screen Now Playing via `MPNowPlayingInfoCenter`, remote commands),
  add them here following the `android-overlay` pattern.
- **No JS bridge.** The Android `window.KahawaiPlayback` bridge exists to
  start/stop a foreground service — iOS has no service, so there is nothing
  for the page to tell native code. `playbackService.ts` already no-ops when
  the bridge is absent, and the audio session is driven from the shell's
  player-state forwarding thread instead.
- **No insets bridge.** WKWebView reports `env(safe-area-inset-*)` correctly,
  so the CSS safe-area variables work natively; `insets.ts` no-ops without
  a bridge.
