// Typed wrappers around the Tauri command bridge. The Rust backend owns
// playback; this module is the only place that talks to it.
// Every call is guarded: in plain `vite dev` (no Tauri webview) the
// commands fail softly and return undefined.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Album,
  AnalogSettings,
  Artist,
  Genre,
  CrossfeedSettings,
  BitPerfectMode,
  DsdStory,
  DopStatus,
  QualityMode,
  DspSettings,
  EqBand,
  OutputDevice,
  PlaybackPrefs,
  PlayerState,
  StreamFormat,
  Track,
} from "./types";

async function cmd<T>(name: string, args?: Record<string, unknown>): Promise<T | undefined> {
  try {
    return await invoke<T>(name, args);
  } catch (err) {
    console.warn(`[tauri] command '${name}' failed:`, err);
    return undefined;
  }
}

/** True inside the Tauri webview (false under plain `vite dev`). */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * URL for a cover through the shell's `artwork://` protocol, which serves
 * from the on-disk cache and fetches from the server only on a miss.
 * (`convertFileSrc` yields the right form per OS, e.g. Windows' http://artwork.localhost.)
 */
export function cachedArtworkUrl(hash: string): string {
  return convertFileSrc(hash, "artwork");
}

export interface ArtworkCacheStats {
  bytes: number;
  files: number;
  max_bytes: number;
  /** Choices for the Settings dropdown (bytes), smallest first. */
  size_options: number[];
  /** Free space on the volume holding the cache dir; null if unreadable. */
  free_bytes: number | null;
}

export async function artworkCacheStats(): Promise<ArtworkCacheStats | undefined> {
  return cmd<ArtworkCacheStats>("artwork_cache_stats");
}

/** Returns how many cached images were deleted. */
export async function clearArtworkCache(): Promise<number | undefined> {
  return cmd<number>("clear_artwork_cache");
}

/** Change the cache's cap; returns the refreshed stats. */
export async function setArtworkCacheMaxBytes(maxBytes: number): Promise<ArtworkCacheStats | undefined> {
  return cmd<ArtworkCacheStats>("set_artwork_cache_max_bytes", { max_bytes: maxBytes });
}

export async function getServerUrl(): Promise<string | undefined> {
  return cmd<string>("get_server_url");
}

export async function setServerUrl(url: string): Promise<void> {
  await cmd("set_server_url", { url });
}

function plain(tracks: Track[]): Track[] {
  // Strip Vue reactivity proxies before crossing the bridge.
  return tracks.map((t) => ({ ...t }));
}

export async function queuePlay(tracks: Track[], index: number): Promise<void> {
  await cmd("queue_play", { tracks: plain(tracks), index });
}

/** Play tracks in order (no repeat, no shuffle) from `index`, `positionMs`
 *  into that track: an audiobook's parts. */
export async function queuePlayAt(tracks: Track[], index: number, positionMs: number): Promise<void> {
  await cmd("queue_play_at", { tracks: plain(tracks), index, positionMs: Math.max(0, Math.round(positionMs)) });
}

/** Put a queue back without starting it (switching back from an audiobook):
 *  the cursor lands on `index`, and play resumes `positionMs` in. */
export async function queueRestore(
  tracks: Track[],
  index: number,
  positionMs: number,
  repeat: RepeatMode,
  shuffle: boolean,
): Promise<void> {
  await cmd("queue_restore", {
    tracks: plain(tracks),
    index,
    positionMs: Math.max(0, Math.round(positionMs)),
    repeat,
    shuffle,
  });
}

/** Reorder the core's queue in place; playback is not interrupted. */
export async function queueMove(from: number, to: number): Promise<void> {
  await cmd("queue_move", { from, to });
}

/** Remove a queue entry in place; playback carries on unless it was playing. */
export async function queueRemove(index: number): Promise<void> {
  await cmd("queue_remove", { index });
}

export async function playTrackById(id: number): Promise<void> {
  await cmd("play_track", { id });
}

export async function pause(): Promise<void> {
  await cmd("pause");
}

export async function resume(): Promise<void> {
  await cmd("resume");
}

export async function toggle(): Promise<void> {
  await cmd("toggle");
}

export async function stop(): Promise<void> {
  await cmd("stop");
}

export async function seekMs(ms: number): Promise<void> {
  await cmd("seek_ms", { ms: Math.max(0, Math.round(ms)) });
}

export async function nextTrack(): Promise<void> {
  await cmd("next_track");
}

export async function prevTrack(): Promise<void> {
  await cmd("prev_track");
}

export async function setFormat(fmt: StreamFormat | null): Promise<void> {
  await cmd("set_format", { fmt });
}

export async function setTrackFormat(trackId: number, fmt: StreamFormat | null): Promise<void> {
  await cmd("set_track_format", { trackId, fmt });
}

export async function setVolume(v: number): Promise<void> {
  await cmd("set_volume", { v: Math.min(1, Math.max(0, v)) });
}

// --- C3: queue actions, repeat/shuffle, DSD story ----------------------------

export type RepeatMode = "off" | "all" | "one";

/** Snapshot the current player state (queue, repeat, shuffle included).
 *  Used at launch so a restored queue hydrates even if the initial
 *  player-state event was emitted before the listener attached. */
export async function getState(): Promise<PlayerState | undefined> {
  return cmd<PlayerState>("get_state");
}

/** The tracks of the queue the engine saved to disk (list order). Empty
 *  outside Tauri. Lets a restored queue show even when the server is down. */
export async function getQueueTracks(): Promise<Track[]> {
  return (await cmd<Track[]>("get_queue_tracks")) ?? [];
}

export async function setRepeat(mode: RepeatMode): Promise<void> {
  await cmd("set_repeat", { mode });
}

export async function setShuffle(on: boolean): Promise<void> {
  await cmd("set_shuffle", { on });
}

/**
 * Strict queue mutations: unlike `cmd()`, these propagate invocation
 * failures so the caller can roll back its optimistic UI update. Used for
 * queue append / play-next, where the Pinia queue store keeps a local
 * optimistic copy.
 */
export async function queueAppendStrict(tracks: Track[]): Promise<void> {
  await invoke("queue_append", { tracks: plain(tracks) });
}

export async function queueInsertNextStrict(tracks: Track[]): Promise<void> {
  await invoke("queue_insert_next", { tracks: plain(tracks) });
}

export async function getPlaybackPrefs(): Promise<PlaybackPrefs | undefined> {
  return cmd<PlaybackPrefs>("get_playback_prefs");
}

/** Exclusive bit-perfect output: "off" | "mqa" | "all". */
export async function setBitPerfect(mode: BitPerfectMode): Promise<void> {
  await cmd("set_bit_perfect", { mode });
}

export async function setDsdStory(story: DsdStory): Promise<void> {
  await cmd("set_dsd_story", { story });
}

/** What the output device is doing right now, read from the OS. */
export interface OutputLive {
  name: string;
  rate_hz: number;
  /** 0 when unknown. */
  bit_depth: number;
  /** The stream format is floating point (the shared mixer's). */
  float: boolean;
  /** This app holds the device exclusively. */
  exclusive: boolean;
}

export async function outputLiveState(): Promise<OutputLive | undefined> {
  return cmd<OutputLive | null>("output_live_state").then((v) => v ?? undefined);
}

/** Top-level quality mode: "best" | "compatible". */
export async function setQualityMode(mode: QualityMode): Promise<void> {
  await cmd("set_quality_mode", { mode });
}

/** Tell "Auto" DSD handling that the current output decodes DoP (or not). */
export async function setDsdDeviceConfirmed(confirmed: boolean): Promise<void> {
  await cmd("set_dsd_device_confirmed", { confirmed });
}

// --- C2: audio devices, DSP, DoP -------------------------------------------

export async function getOutputDevices(): Promise<OutputDevice[] | undefined> {
  return cmd<OutputDevice[]>("get_output_devices");
}

/** The chosen output device by exact name; null = follow the system default. */
export async function getOutputDevice(): Promise<string | null> {
  return (await cmd<string | null>("get_output_device")) ?? null;
}

export async function setOutputDevice(name: string | null): Promise<void> {
  await cmd("set_output_device", { name });
}

export async function setEqBands(bands: EqBand[]): Promise<void> {
  await cmd("set_eq_bands", { bands });
}

export async function setEqEnabled(enabled: boolean): Promise<void> {
  await cmd("set_eq_enabled", { enabled });
}

/** Playback speed, 0.5 to 3.0, pitch kept. Not saved: the book being played decides it. */
export async function setPlaybackRate(rate: number): Promise<void> {
  await cmd("set_playback_rate", { rate });
}

/** The EQ's preamp in dB (headroom for its boosts); in effect only while the EQ is on. */
export async function setEqPreamp(db: number): Promise<void> {
  await cmd("set_eq_preamp", { db });
}

export async function setAnalog(settings: AnalogSettings): Promise<void> {
  await cmd("set_analog", { settings });
}

export async function setLoudnessTarget(lufs: number): Promise<void> {
  await cmd("set_loudness_target", { lufs });
}

export async function setLoudnessEnabled(enabled: boolean): Promise<void> {
  await cmd("set_loudness_enabled", { enabled });
}

export async function setLimiterEnabled(enabled: boolean): Promise<void> {
  await cmd("set_limiter_enabled", { enabled });
}

/** Headphone crossfeed; the engine clamps, saves and applies it live. */
export async function setCrossfeed(settings: CrossfeedSettings): Promise<void> {
  await cmd("set_crossfeed", { settings });
}

export async function getDspSettings(): Promise<DspSettings | undefined> {
  return cmd<DspSettings>("get_dsp_settings");
}

export async function dopStatus(): Promise<DopStatus | undefined> {
  return cmd<DopStatus>("dop_status");
}

/**
 * Subscribe to the engine's `player-state` events. Returns `null` when the
 * subscription is refused (e.g. missing `core:event` capability), so the
 * caller can fall back to polling instead of silently going stale.
 */
/**
 * Native menu selections. The shell owns no behaviour: each item just forwards
 * its id (for example "app.about") and the UI decides what it does.
 */
export async function onMenuAction(cb: (id: string) => void): Promise<UnlistenFn | null> {
  if (!inTauri()) return null;
  try {
    return await listen<string>("menu-action", (event) => cb(event.payload));
  } catch (err) {
    console.error("[tauri] could not subscribe to menu-action:", err);
    return null;
  }
}

export async function onPlayerState(cb: (s: PlayerState) => void): Promise<UnlistenFn | null> {
  try {
    return await listen<PlayerState>("player-state", (event) => cb(event.payload));
  } catch (err) {
    console.error("[tauri] could not subscribe to player-state:", err);
    return null;
  }
}

// --- Catalog cache (docs/v1/kahawai-player-catalog-cache-spec.md) ------------

/** The library as last synced, straight from the shell's cache file. */
export interface CachedCatalog {
  /** `null` until the cache has been filled once. */
  rev: number | null;
  albums: Album[];
  artists: Artist[];
  genres: Genre[];
}

export type CatalogSyncStatus = "unchanged" | "updated" | "full" | "offline" | "unsupported";

export interface CatalogSyncReport {
  status: CatalogSyncStatus;
  changed: number;
  message?: string | null;
}

/** `undefined` outside Tauri or when the command fails. */
export async function catalogCached(): Promise<CachedCatalog | undefined> {
  return inTauri() ? cmd<CachedCatalog>("catalog_cached") : undefined;
}

/** Bring the cache up to date with the server. */
export async function catalogSync(): Promise<CatalogSyncReport | undefined> {
  return inTauri() ? cmd<CatalogSyncReport>("catalog_sync") : undefined;
}

export async function catalogAlbumTracks(albumId: number): Promise<Track[] | undefined> {
  return inTauri() ? cmd<Track[]>("catalog_album_tracks", { albumId }) : undefined;
}

export async function catalogTracks(ids: number[]): Promise<Track[] | undefined> {
  return inTauri() ? cmd<Track[]>("catalog_tracks", { ids }) : undefined;
}

// --- Developer tools (Settings) ----------------------------------------------

export interface DeveloperTools {
  enabled: boolean;
  /** This window has the Web Inspector (a change applies after a restart). */
  inspector: boolean;
  dev_build: boolean;
}

export async function getDeveloperTools(): Promise<DeveloperTools | undefined> {
  return cmd<DeveloperTools>("get_developer_tools");
}

export async function setDeveloperTools(enabled: boolean): Promise<DeveloperTools | undefined> {
  return cmd<DeveloperTools>("set_developer_tools", { enabled });
}

// --- UI state (ui-state.json; see lib/uiState.ts) ------------------------------

export async function getUiState(): Promise<Record<string, unknown> | undefined> {
  return cmd<Record<string, unknown>>("get_ui_state");
}

export async function setUiState(key: string, value: string | null): Promise<void> {
  await cmd("set_ui_state", { key, value });
}

/** The UI is warmed up: the shell swaps its splash window for this one. */
export async function appReady(): Promise<void> {
  if (inTauri()) await cmd("app_ready");
}

/** Show the log folder (~/Library/Logs/Kahawai Player) in Finder. */
export async function revealLogs(): Promise<void> {
  if (inTauri()) await cmd("reveal_logs");
}

/** Open a web or mail link in the default browser (outside the app: show notes, a show's site). */
export async function openUrl(url: string): Promise<void> {
  if (inTauri()) await cmd("open_url", { url });
  else window.open(url, "_blank", "noopener");
}
