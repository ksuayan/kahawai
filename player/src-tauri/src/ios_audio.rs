//! iOS background-audio session management.
//!
//! The iOS equivalent of the Android foreground service + wake/Wi-Fi locks
//! (`PlaybackService.kt`). iOS needs no service: audio keeps playing in the
//! background when (a) the app declares the `audio` background mode in
//! Info.plist (see `scripts/ios-plist-overlay.py`) and (b) an `AVAudioSession`
//! with the Playback category is active.
//!
//! Driven from the shell's player-state forwarding thread
//! (`player/src-tauri/src/lib.rs`): `set_playing(true)` when the engine
//! starts playing, `set_playing(false)` when it stops. Activating only while
//! playing is the polite pattern — a permanently active session would hold
//! audio focus and interrupt other apps. The category is set once; only the
//! active flag toggles per play cycle.
//!
//! NOTE: written against `objc2-avf-audio` 0.3; not compiled in this
//! environment (no Xcode/iOS SDK here). Verify on the first
//! `scripts/build-ios.sh` run: the method names are the standard
//! `setCategory:error:` / `setActive:error:` selectors.

use objc2_avf_audio::{AVAudioSession, AVAudioSessionCategoryPlayback};
use std::sync::OnceLock;

/// The shared session, fetched once.
fn session() -> &'static AVAudioSession {
    static SESSION: OnceLock<&'static AVAudioSession> = OnceLock::new();
    SESSION.get_or_init(|| AVAudioSession::sharedInstance())
}

/// Set the Playback category once. Called on the first `set_playing(true)`;
/// failures are logged and retried next time (a denied category fails the
/// later activation too, which is also logged).
fn ensure_category() {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.get().is_some() {
        return;
    }
    // SAFETY: AVAudioSession methods are safe to call from any thread;
    // setCategory:error: is a plain Objective-C message send.
    match session().setCategory_error(AVAudioSessionCategoryPlayback) {
        Ok(()) => {
            let _ = DONE.set(());
        }
        Err(e) => {
            tracing::warn!(error = %e, "ios: could not set audio session category");
        }
    }
}

/// Activate (`playing = true`) or deactivate the audio session.
/// Fire-and-forget: errors are logged, playback itself is unaffected.
pub fn set_playing(playing: bool) {
    if playing {
        ensure_category();
    }
    // SAFETY: as above.
    if let Err(e) = session().setActive_error(playing) {
        tracing::warn!(playing, error = %e, "ios: could not set audio session active");
    } else {
        tracing::debug!(playing, "ios: audio session active = {playing}");
    }
}
