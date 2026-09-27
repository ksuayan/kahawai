import { flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { makeState, makeTrack, mockFetch } from "../test/fixtures";
import { tauri } from "../test/tauri-mock";
import { useLibraryStore } from "./library";
import { usePlayerStore } from "./player";
import { useQueueStore } from "./queue";

const a = makeTrack({ title: "A" });
const b = makeTrack({ title: "B" });
const c = makeTrack({ title: "C" });
const d = makeTrack({ title: "D" });
const titles = (q: ReturnType<typeof useQueueStore>) => q.tracks.map((t) => t.title);

beforeEach(() => setActivePinia(createPinia()));

describe("queue store", () => {
  it("playAll adopts the list and starts playing from the chosen index", async () => {
    const q = useQueueStore();
    await q.playAll([a, b, c], 1);
    expect(titles(q)).toEqual(["A", "B", "C"]);
    expect(q.index).toBe(1);
    expect(q.current?.title).toBe("B");
    expect(q.queueIds).toEqual([a.id, b.id, c.id]);
    const call = tauri.callsTo("queue_play")[0] as { tracks: { id: number }[]; index: number };
    expect(call.index).toBe(1);
    expect(call.tracks.map((t) => t.id)).toEqual([a.id, b.id, c.id]);
  });

  it("copies tracks so later edits to a source list do not leak in", async () => {
    const q = useQueueStore();
    const src = [{ ...a }];
    await q.playAll(src, 0);
    src[0].title = "mutated";
    expect(q.tracks[0].title).toBe("A");
  });

  describe("playAt", () => {
    it("moves the cursor and restarts playback from that index, keeping the whole queue", async () => {
      const q = useQueueStore();
      q.tracks = [a, b, c].map((t) => ({ ...t }));
      q.index = 0;
      await q.playAt(2);
      expect(q.index).toBe(2);
      const call = tauri.callsTo("queue_play")[0] as { tracks: { id: number }[]; index: number };
      expect(call.index).toBe(2);
      expect(call.tracks).toHaveLength(3);
    });

    it("ignores an index outside the queue", async () => {
      const q = useQueueStore();
      q.tracks = [{ ...a }];
      q.index = 0;
      await q.playAt(5);
      await q.playAt(-1);
      expect(q.index).toBe(0);
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
    });
  });

  describe("syncFromState (the engine is the truth)", () => {
    it("adopts the engine's queue order and cursor", async () => {
      const lib = useLibraryStore();
      lib.cacheTracks([a, b, c]);
      const q = useQueueStore();
      await q.syncFromState(makeState({ queue_ids: [c.id, a.id, b.id], queue_index: 2 }));
      expect(titles(q)).toEqual(["C", "A", "B"]);
      expect(q.index).toBe(2);
    });

    it("hydrates unknown ids from the server", async () => {
      mockFetch({ [`/api/tracks/${d.id}`]: d });
      const q = useQueueStore();
      await q.syncFromState(makeState({ queue_ids: [d.id], queue_index: 0 }));
      expect(titles(q)).toEqual(["D"]);
    });

    it("keeps what it has when hydration fails, still moving the cursor", async () => {
      mockFetch({});
      const q = useQueueStore();
      q.tracks = [a, b];
      await q.syncFromState(makeState({ queue_ids: [a.id, b.id, 999], queue_index: 1 }));
      expect(q.index).toBe(1);
    });

    it("does not refetch when the queue is unchanged", async () => {
      const calls = mockFetch({});
      const q = useQueueStore();
      q.tracks = [a, b];
      await q.syncFromState(makeState({ queue_ids: [a.id, b.id], queue_index: 0 }));
      expect(calls).toHaveLength(0);
    });

    it("clamps an out-of-range cursor, and clears it for an empty queue", async () => {
      const q = useQueueStore();
      q.tracks = [a, b];
      await q.syncFromState(makeState({ queue_ids: [a.id, b.id], queue_index: 9 }));
      expect(q.index).toBe(1);
      await q.syncFromState(makeState({ track: null, queue_ids: [], queue_index: null }));
      expect(q.tracks).toEqual([]);
      expect(q.index).toBeNull();
    });
  });

  describe("append and play next (optimistic, rolled back on failure)", () => {
    it("appends to the end", async () => {
      const q = useQueueStore();
      q.tracks = [a];
      await q.appendTracks([b, c]);
      expect(titles(q)).toEqual(["A", "B", "C"]);
      expect(tauri.callsTo("queue_append")).toHaveLength(1);
    });

    it("inserts right after the current track", async () => {
      const q = useQueueStore();
      q.tracks = [a, b, c];
      q.index = 0;
      await q.playNext([d]);
      expect(titles(q)).toEqual(["A", "D", "B", "C"]);
    });

    it("plays next at the end when nothing is current", async () => {
      const q = useQueueStore();
      q.tracks = [a];
      await q.playNext([b]);
      expect(titles(q)).toEqual(["A", "B"]);
    });

    it("ignores an empty list", async () => {
      const q = useQueueStore();
      await q.appendTracks([]);
      await q.playNext([]);
      expect(tauri.calls).toHaveLength(0);
    });

    it("restores the exact previous queue when the engine refuses", async () => {
      tauri.on("queue_append", () => { throw new Error("no"); });
      tauri.on("queue_insert_next", () => { throw new Error("no"); });
      const q = useQueueStore();
      q.tracks = [a, a]; // duplicates must survive the rollback
      await expect(q.appendTracks([b])).rejects.toThrow("no");
      expect(titles(q)).toEqual(["A", "A"]);
      await expect(q.playNext([c])).rejects.toThrow("no");
      expect(titles(q)).toEqual(["A", "A"]);
    });
  });

  describe("reorder / remove / clear", () => {
    async function withQueue(index = 0) {
      const q = useQueueStore();
      q.tracks = [a, b, c, d].map((t) => ({ ...t }));
      q.index = index;
      return q;
    }

    it("reorder moves a track and keeps the playing one current", async () => {
      const q = await withQueue(1); // B is playing
      await q.reorder(0, 3);
      expect(titles(q)).toEqual(["B", "C", "D", "A"]);
      expect(q.current?.title).toBe("B");
      expect(q.index).toBe(0);
      await flushPromises();
      // Applied in place in the core: nothing restarts.
      expect(tauri.callsTo("queue_move")).toEqual([{ from: 0, to: 3 }]);
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
    });

    it("reorder ignores a no-op or out-of-range move", async () => {
      const q = await withQueue();
      await q.reorder(1, 1);
      await q.reorder(-1, 2);
      await q.reorder(0, 9);
      expect(titles(q)).toEqual(["A", "B", "C", "D"]);
      expect(tauri.callsTo("queue_move")).toHaveLength(0);
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
    });

    it("moveUp / moveDown stop at the ends", async () => {
      const q = await withQueue();
      await q.moveUp(0);
      await q.moveDown(3);
      expect(titles(q)).toEqual(["A", "B", "C", "D"]);
      await q.moveDown(0);
      expect(titles(q)).toEqual(["B", "A", "C", "D"]);
      await q.moveUp(3);
      expect(titles(q)).toEqual(["B", "A", "D", "C"]);
    });

    it("removing before the current track shifts the cursor; removing the current keeps a valid one", async () => {
      const q = await withQueue(2); // C
      await q.removeAt(0);
      expect(q.current?.title).toBe("C");
      await q.removeAt(1); // current
      expect(q.index).toBe(1);
      expect(q.current?.title).toBe("D");
      await q.removeAt(1); // last, and current
      expect(q.index).toBe(0);
    });

    it("removing the only track leaves the core to stop playback", async () => {
      const q = useQueueStore();
      q.tracks = [a];
      q.index = 0;
      await q.removeAt(0);
      expect(q.index).toBeNull();
      expect(tauri.callsTo("queue_remove")).toEqual([{ index: 0 }]);
    });

    it("removing sends the list position to the core without restarting anything", async () => {
      const q = await withQueue(2);
      await q.removeAt(0);
      expect(tauri.callsTo("queue_remove")).toEqual([{ index: 0 }]);
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
      expect(tauri.callsTo("seek_ms")).toHaveLength(0);
    });

    it("ignores an out-of-range remove", async () => {
      const q = await withQueue();
      await q.removeAt(9);
      await q.removeAt(-1);
      expect(q.tracks).toHaveLength(4);
    });

    it("clear empties the queue in the core too, so the next sync can't restore it", async () => {
      const q = await withQueue();
      tauri.reset?.();
      await q.clear();
      expect(q.tracks).toEqual([]);
      expect(q.current).toBeNull();
      expect(tauri.callsTo("queue_play")).toEqual([{ tracks: [], index: 0 }]);
    });

    it("a reorder does not restart or seek the playing track", async () => {
      tauri.on("get_state", makeState({ status: "playing", position_ms: 45_000 }));
      const player = usePlayerStore();
      await player.init();
      const q = await withQueue();
      await q.reorder(2, 3);
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
      expect(tauri.callsTo("seek_ms")).toHaveLength(0);
    });
  });
});
