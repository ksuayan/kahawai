/**
 * Background-audio keep-alive on Android
 * (player/src-tauri/android-overlay's PlaybackService).
 *
 * Without a foreground service, Android treats the player as a background app
 * once the screen goes off and Doze / app-standby throttling starves the Rust
 * audio thread: playback breaks up and stutters after some minutes (seen on
 * the HiBy R4). The native side exposes
 * `window.KahawaiPlayback.setActive(active, title, artist)`; the player store
 * calls it whenever playback starts or stops, and the service (with a partial
 * wake lock) keeps the process exempt from throttling while playing.
 *
 * Elsewhere (the desktop, a browser) there is no bridge and nothing happens.
 */
interface Bridge {
  setActive(active: boolean, title: string | null, artist: string | null): void;
}

function bridge(): Bridge | null {
  const b = (window as unknown as { KahawaiPlayback?: Bridge }).KahawaiPlayback;
  return b && typeof b.setActive === "function" ? b : null;
}

/** True on the Android app, where the bridge exists. */
export function hasPlaybackBridge(): boolean {
  return bridge() !== null;
}

/**
 * Tell the Android service whether audio is playing. Fire-and-forget: the
 * native side owns the service lifecycle, and a missed call only leaves the
 * notification up until the next transition.
 */
export function notifyPlaybackState(
  active: boolean,
  title?: string | null,
  artist?: string | null,
): void {
  const b = bridge();
  if (!b) return;
  try {
    b.setActive(active, title ?? null, artist ?? null);
  } catch {
    // The bridge is gone (WebView torn down); nothing to keep alive.
  }
}
