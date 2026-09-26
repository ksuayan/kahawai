// Typed wrappers around the Tauri command bridge. The Rust backend owns
// playback; this module is the only place that talks to it.
// Every call is guarded: in plain `vite dev` (no Tauri webview) the
// commands fail softly and return undefined.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DsdStory,
  DopStatus,
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
  await cmd("set_track_format", { track_id: trackId, fmt });
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

export async function setDsdStory(story: DsdStory): Promise<void> {
  await cmd("set_dsd_story", { story });
}

// --- C2: audio devices, DSP, DoP -------------------------------------------

export async function getOutputDevices(): Promise<OutputDevice[] | undefined> {
  return cmd<OutputDevice[]>("get_output_devices");
}

export async function setEqBands(bands: EqBand[]): Promise<void> {
  await cmd("set_eq_bands", { bands });
}

export async function setEqEnabled(enabled: boolean): Promise<void> {
  await cmd("set_eq_enabled", { enabled });
}

export async function setLoudnessTarget(lufs: number): Promise<void> {
  await cmd("set_loudness_target", { lufs });
}

export async function setLoudnessEnabled(enabled: boolean): Promise<void> {
  await cmd("set_loudness_enabled", { enabled });
}

export async function getDspSettings(): Promise<DspSettings | undefined> {
  return cmd<DspSettings>("get_dsp_settings");
}

export async function dopStatus(): Promise<DopStatus | undefined> {
  return cmd<DopStatus>("dop_status");
}

export async function onPlayerState(cb: (s: PlayerState) => void): Promise<UnlistenFn> {
  try {
    return await listen<PlayerState>("player-state", (event) => cb(event.payload));
  } catch (err) {
    console.warn("[tauri] could not subscribe to player-state:", err);
    return () => {};
  }
}
