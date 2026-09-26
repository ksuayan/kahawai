import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { queuePlay, seekMs, stop } from "../tauri";
import { useLibraryStore } from "./library";
import { usePlayerStore } from "./player";
import type { PlayerState, Track } from "../types";

/**
 * The visible play queue, held authoritatively here as Track[].
 *
 * The Rust core owns the real queue (and persists it across launches);
 * this store mirrors it. `syncFromState` reconciles the local list with
 * the core's `queue_ids` on every player-state event, hydrating unknown
 * ids through the library cache — so core-side mutations (append,
 * play-next, launch restore) show up here without a restart.
 *
 * Local reorder/remove still re-sync via `queue_play` (the core has no
 * in-place reorder command): this restarts the current track and
 * re-seeks as an approximation.
 */
export const useQueueStore = defineStore("queue", () => {
  const tracks = ref<Track[]>([]);
  const index = ref<number | null>(null);

  const current = computed(() =>
    index.value != null ? tracks.value[index.value] ?? null : null,
  );
  const queueIds = computed(() => tracks.value.map((t) => t.id));

  /** Reconcile with the core: adopt its queue order, hydrate unknown ids,
   *  and move the index. Never invents state — the core is the truth. */
  async function syncFromState(s: PlayerState): Promise<void> {
    const ids = s.queue_ids;
    const localIds = tracks.value.map((t) => t.id);
    const same =
      ids.length === localIds.length && ids.every((id, i) => id === localIds[i]);
    if (!same) {
      const lib = useLibraryStore();
      try {
        tracks.value = await lib.ensureTracks(ids);
      } catch (e) {
        console.warn("[queue] could not hydrate queue tracks:", e);
      }
    }
    if (s.queue_index != null) {
      index.value =
        tracks.value.length > 0
          ? Math.min(Math.max(0, s.queue_index), tracks.value.length - 1)
          : null;
    } else if (tracks.value.length === 0) {
      index.value = null;
    }
  }

  async function playAll(list: Track[], startIndex: number): Promise<void> {
    tracks.value = list.map((t) => ({ ...t }));
    index.value = startIndex;
    await queuePlay(tracks.value, startIndex);
  }

  /** Append to the end of the queue ("add to queue"). The core command
   *  doesn't disturb playback; the local list updates optimistically and
   *  `syncFromState` confirms it when the echo arrives. */
  async function appendTracks(list: Track[]): Promise<void> {
    if (list.length === 0) return;
    const player = usePlayerStore();
    const prev = tracks.value;
    tracks.value = [...tracks.value, ...list.map((t) => ({ ...t }))];
    try {
      await player.appendTracks(list);
    } catch (e) {
      // Roll back to the exact prior queue (ID filtering could drop
      // pre-existing duplicates of the appended tracks).
      tracks.value = prev;
      throw e;
    }
  }

  /** Insert right after the current item ("play next"), optimistically. */
  async function playNext(list: Track[]): Promise<void> {
    if (list.length === 0) return;
    const player = usePlayerStore();
    const at = index.value != null ? index.value + 1 : tracks.value.length;
    const prev = tracks.value;
    tracks.value = [
      ...tracks.value.slice(0, at),
      ...list.map((t) => ({ ...t })),
      ...tracks.value.slice(at),
    ];
    try {
      await player.playNext(list);
    } catch (e) {
      tracks.value = prev;
      throw e;
    }
  }

  /** Re-sync the core after a local mutation (see C1 note above). */
  async function resync(preservePosition: boolean): Promise<void> {
    const player = usePlayerStore();
    const pos = player.positionMs;
    const cur = index.value ?? 0;
    await queuePlay(tracks.value, cur);
    if (preservePosition && pos > 1000) await seekMs(pos);
  }

  async function reorder(from: number, to: number): Promise<void> {
    if (from === to) return;
    if (from < 0 || to < 0 || from >= tracks.value.length || to >= tracks.value.length) return;
    const currentId = index.value != null ? tracks.value[index.value]?.id : undefined;
    const [item] = tracks.value.splice(from, 1);
    if (!item) return;
    tracks.value.splice(to, 0, item);
    if (currentId !== undefined) {
      const ni = tracks.value.findIndex((t) => t.id === currentId);
      if (ni >= 0) index.value = ni;
    }
    await resync(true);
  }

  async function moveUp(i: number): Promise<void> {
    if (i > 0) await reorder(i, i - 1);
  }

  async function moveDown(i: number): Promise<void> {
    if (i < tracks.value.length - 1) await reorder(i, i + 1);
  }

  async function removeAt(i: number): Promise<void> {
    if (i < 0 || i >= tracks.value.length) return;
    tracks.value.splice(i, 1);
    if (index.value != null) {
      if (tracks.value.length === 0) {
        index.value = null;
        await stop();
        return;
      }
      if (i < index.value) index.value -= 1;
      else if (i === index.value) index.value = Math.min(index.value, tracks.value.length - 1);
    }
    await resync(true);
  }

  async function clear(): Promise<void> {
    tracks.value = [];
    index.value = null;
    await stop();
  }

  return {
    tracks,
    index,
    current,
    queueIds,
    syncFromState,
    playAll,
    appendTracks,
    playNext,
    reorder,
    moveUp,
    moveDown,
    removeAt,
    clear,
  };
});
