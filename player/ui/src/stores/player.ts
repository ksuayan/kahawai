import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  getState as fetchState,
  nextTrack,
  onPlayerState,
  pause,
  prevTrack,
  queueAppendStrict,
  queueInsertNextStrict,
  resume,
  seekMs,
  setRepeat,
  setShuffle,
  setTrackFormat,
  setVolume,
  stop,
  toggle,
  type RepeatMode,
} from "../tauri";
import type { PlayerState, StreamFormat, Track } from "../types";

/**
 * Mirrors the Rust core's playback state via the `player-state` event.
 * The core is the single source of truth; this store never invents state.
 */
export const usePlayerStore = defineStore("player", () => {
  const raw = ref<PlayerState | null>(null);
  const connected = ref(false);
  /** Wall-clock ms when the last player-state event arrived. */
  const lastEventAt = ref(0);
  const tick = ref(0);

  let timer: number | undefined;
  let pollTimer: number | undefined;
  /**
   * A scrub the engine has not confirmed yet. Events already in flight when
   * the user releases the slider still carry the OLD playhead; without this
   * the bar would jump back for a moment (or for the whole download if the
   * seek needs a fresh stream).
   */
  let pendingSeek: { ms: number; until: number } | null = null;
  const SEEK_GUARD_MS = 5000;

  function applySeekGuard(s: PlayerState): PlayerState {
    if (!pendingSeek) return s;
    if (Date.now() > pendingSeek.until) {
      pendingSeek = null;
      return s;
    }
    if (Math.abs(s.position_ms - pendingSeek.ms) <= 1500) {
      pendingSeek = null; // the engine caught up with the target
      return s;
    }
    return { ...s, position_ms: pendingSeek.ms };
  }

  async function init(): Promise<void> {
    const accept = (incoming: PlayerState): void => {
      raw.value = applySeekGuard(incoming);
      lastEventAt.value = Date.now();
      connected.value = true;
    };
    const unlisten = await onPlayerState(accept);
    if (unlisten === null && !!(await fetchState())) {
      // Events are unavailable: poll the engine snapshot (refreshed ~4 Hz
      // while playing) so the title, artwork and playhead still go live.
      pollTimer = window.setInterval(async () => {
        const s = await fetchState();
        if (s) accept(s);
      }, 300);
    }
    // The shell may have emitted the initial state before the listener
    // attached (a queue restored from disk would be missed). Fetch it
    // directly so launch state always hydrates.
    const initial = await fetchState();
    if (initial) {
      raw.value = initial;
      lastEventAt.value = Date.now();
      connected.value = true;
    }
    // Local tick so the seek bar advances smoothly between events.
    timer = window.setInterval(() => {
      tick.value += 1;
    }, 500);
  }

  function dispose(): void {
    window.clearInterval(timer);
    window.clearInterval(pollTimer);
  }

  const status = computed(() => raw.value?.status ?? "stopped");
  const currentTrack = computed(() => raw.value?.track ?? null);
  const isPlaying = computed(() => status.value === "playing");
  const isLoading = computed(() => status.value === "loading");
  const durationMs = computed(() => raw.value?.duration_ms ?? null);
  const bufferedMs = computed(() => raw.value?.buffered_ms ?? null);
  const outputRateHz = computed(() => raw.value?.output_rate_hz ?? null);
  const chain = computed(() => raw.value?.chain ?? null);
  const activeFormat = computed(() => raw.value?.format ?? null);
  const outputPath = computed(() => raw.value?.output_path ?? "pcm-shared");
  const isDopExclusive = computed(() => outputPath.value === "dop-exclusive");
  /** Exclusive PCM: the file's own samples, untouched (MQA DACs, audiophile mode). */
  const isBitPerfect = computed(() => outputPath.value === "pcm-exclusive");
  /** Either exclusive path: EQ, loudness and software volume are bypassed. */
  const isExclusive = computed(() => isDopExclusive.value || isBitPerfect.value);
  const volume = computed(() => raw.value?.volume ?? 1);
  const error = computed(() => raw.value?.error ?? null);
  const repeat = computed<RepeatMode>(() => raw.value?.repeat ?? "off");
  const shuffle = computed(() => raw.value?.shuffle ?? false);

  /** Interpolated position between core events; clamps at duration. */
  const positionMs = computed(() => {
    void tick.value;
    const s = raw.value;
    if (!s) return 0;
    let pos = s.position_ms;
    if (s.status === "playing") pos += Date.now() - lastEventAt.value;
    if (s.duration_ms != null) pos = Math.min(pos, s.duration_ms);
    return Math.max(0, Math.floor(pos));
  });

  async function seekTo(ms: number): Promise<void> {
    // Optimistic local update so the bar doesn't snap back mid-drag.
    if (raw.value) raw.value = { ...raw.value, position_ms: Math.round(ms) };
    lastEventAt.value = Date.now();
    pendingSeek = { ms: Math.round(ms), until: Date.now() + SEEK_GUARD_MS };
    await seekMs(ms);
  }

  async function changeVolume(v: number): Promise<void> {
    // Optimistic: the slider must not wait for the round trip (or be
    // yanked back by an event that predates this change).
    if (raw.value) raw.value = { ...raw.value, volume: v };
    await setVolume(v);
  }

  /**
   * Formats the user forced for individual tracks this session. The engine
   * keeps the same map in memory (it is not persisted); mirroring it here
   * lets the picker say "Auto" until a track has really been overridden,
   * instead of showing whatever format happens to be playing.
   */
  const trackOverrides = ref<Record<number, StreamFormat>>({});

  function formatOverride(trackId: number | null | undefined): StreamFormat | null {
    return trackId == null ? null : (trackOverrides.value[trackId] ?? null);
  }

  async function changeTrackFormat(trackId: number, fmt: StreamFormat | null): Promise<void> {
    if (fmt === null) delete trackOverrides.value[trackId];
    else trackOverrides.value[trackId] = fmt;
    await setTrackFormat(trackId, fmt);
  }

  /** Cycle repeat off → all → one → off. */
  async function cycleRepeat(): Promise<void> {
    const next: RepeatMode =
      repeat.value === "off" ? "all" : repeat.value === "all" ? "one" : "off";
    // Optimistic: the core echoes the mode back via player-state.
    if (raw.value) raw.value = { ...raw.value, repeat: next };
    await setRepeat(next);
  }

  async function toggleShuffle(): Promise<void> {
    const next = !shuffle.value;
    if (raw.value) raw.value = { ...raw.value, shuffle: next };
    await setShuffle(next);
  }

  /** Append tracks to the queue without disturbing playback. */
  async function appendTracks(tracks: Track[]): Promise<void> {
    if (tracks.length === 0) return;
    await queueAppendStrict(tracks);
  }

  /** Insert tracks right after the current queue item ("play next"). */
  async function playNext(tracks: Track[]): Promise<void> {
    if (tracks.length === 0) return;
    await queueInsertNextStrict(tracks);
  }

  return {
    raw,
    connected,
    status,
    currentTrack,
    isPlaying,
    isLoading,
    positionMs,
    durationMs,
    bufferedMs,
    outputRateHz,
    chain,
    activeFormat,
    outputPath,
    isDopExclusive,
    isBitPerfect,
    isExclusive,
    volume,
    error,
    repeat,
    shuffle,
    init,
    dispose,
    toggle,
    pause,
    resume,
    stop,
    nextTrack,
    prevTrack,
    seekTo,
    changeVolume,
    changeTrackFormat,
    formatOverride,
    cycleRepeat,
    toggleShuffle,
    appendTracks,
    playNext,
  };
});
