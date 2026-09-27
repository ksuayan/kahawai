import { describe, expect, it } from "vitest";
import { watch } from "vue";
import { makeState, makeTrack } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useLibraryStore } from "../stores/library";
import { usePlayerStore } from "../stores/player";
import { useQueueStore } from "../stores/queue";
import QueueView from "./QueueView.vue";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * The queue against a core that behaves like the real one: it applies edits
 * after a delay, and keeps emitting its (old) state in the meantime.
 */
describe("queue edits are applied in place and never restart playback", () => {
  it("move and remove converge, without flicker, and never call queue_play or seek", async () => {
    const tracks = [1, 2, 3, 4, 5].map((n) => makeTrack({ id: n, title: `T${n}`, format: "mp3", sample_rate: 44100 }));
    let ids = tracks.map((t) => t.id);
    let index = 0;
    const state = () =>
      makeState({ status: "playing", queue_ids: ids, queue_index: index, track: tracks.find((t) => t.id === ids[index]) ?? null, position_ms: 30_000 });
    const APPLY_MS = 150; // the engine thread is busy: stale echoes arrive meanwhile
    tauri.on("get_state", () => state());
    tauri.on("get_queue_tracks", () => tracks);
    tauri.on("queue_move", async (a?: Record<string, unknown>) => {
      const { from, to } = a as { from: number; to: number };
      await sleep(APPLY_MS);
      const cur = ids[index];
      const [item] = ids.splice(from, 1);
      ids.splice(to, 0, item!);
      index = ids.indexOf(cur!);
      tauri.emit("player-state", state());
    });
    tauri.on("queue_remove", async (a?: Record<string, unknown>) => {
      const { index: i } = a as { index: number };
      await sleep(APPLY_MS);
      const cur = ids[index];
      ids.splice(i, 1);
      index = Math.max(0, ids.indexOf(cur!));
      tauri.emit("player-state", state());
    });
    const ticker = setInterval(() => tauri.emit("player-state", state()), 40);

    const { wrapper } = mountApp(QueueView, {}, {}, () => useLibraryStore().cacheTracks(tracks));
    const player = usePlayerStore();
    const queue = useQueueStore();
    watch(() => player.raw, (s) => s && void queue.syncFromState(s));
    await player.init();
    await settle();
    await sleep(120);
    expect(queue.tracks.map((t) => t.id)).toEqual([1, 2, 3, 4, 5]);

    // Record every order the UI shows, to catch a revert to the old order.
    const seen: string[] = [];
    watch(() => queue.tracks.map((t) => t.id).join(","), (v) => seen.push(v), { flush: "post" });

    const click = async (i: number, label: string) => {
      await wrapper.findAll('[data-testid="queue-row"]')[i]!.get(`button[aria-label="${label}"]`).trigger("click");
      await sleep(APPLY_MS + 250);
    };
    await click(2, "Move up"); // [1,3,2,4,5]
    expect(queue.tracks.map((t) => t.id)).toEqual([1, 3, 2, 4, 5]);
    await click(0, "Move down"); // [3,1,2,4,5]
    expect(queue.tracks.map((t) => t.id)).toEqual([3, 1, 2, 4, 5]);
    await click(1, "Remove from queue"); // [3,2,4,5]
    expect(queue.tracks.map((t) => t.id)).toEqual([3, 2, 4, 5]);
    clearInterval(ticker);

    const cmds = tauri.calls.map((c) => c.cmd);
    expect(cmds).not.toContain("queue_play");
    expect(cmds).not.toContain("seek_ms");
    expect(cmds.filter((c) => c === "queue_move")).toHaveLength(2);
    expect(cmds.filter((c) => c === "queue_remove")).toHaveLength(1);
    // Each edit showed up once and stayed: no bounce back to the previous order.
    expect(seen).toEqual(["1,3,2,4,5", "3,1,2,4,5", "3,2,4,5"]);
    expect(wrapper.findAll('[data-testid="queue-row"]')).toHaveLength(4);
  }, 20000);

  it("does not get stuck ignoring the core if an edit never lands", async () => {
    const tracks = [1, 2, 3].map((n) => makeTrack({ id: n, title: `T${n}`, format: "mp3" }));
    const ids = [1, 2, 3];
    tauri.on("get_queue_tracks", () => tracks);
    tauri.on("queue_remove", () => undefined); // the core never applies it
    const { wrapper } = mountApp(QueueView, {}, {}, () => useLibraryStore().cacheTracks(tracks));
    const queue = useQueueStore();
    queue.tracks = tracks.map((t) => ({ ...t }));
    queue.index = 0;
    await settle();
    await wrapper.findAll('[data-testid="queue-row"]')[1]!.get('button[aria-label="Remove from queue"]').trigger("click");
    await settle();
    expect(queue.tracks.map((t) => t.id)).toEqual([1, 3]);
    // The core still says [1,2,3]: ignored while we wait for our echo...
    await queue.syncFromState(makeState({ queue_ids: ids, queue_index: 0 }));
    expect(queue.tracks.map((t) => t.id)).toEqual([1, 3]);
  });
});
