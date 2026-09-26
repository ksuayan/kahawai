import { describe, expect, it } from "vitest";
import { makeState, makeTrack, mockFetch } from "../test/fixtures";
import { $$, bodyOf, dialog, mountApp, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "../stores/player";
import { useQueueStore } from "../stores/queue";
import QueueView from "./QueueView.vue";

const a = makeTrack({ title: "Alpha", artist: "AA" });
const b = makeTrack({ title: "Beta", artist: "BB" });
const c = makeTrack({ title: "Gamma", artist: null as never });

async function mountQueue(tracks = [a, b, c], index: number | null = 0) {
  const { wrapper } = mountApp(QueueView, {}, {}, () => {
    const q = useQueueStore();
    q.tracks = tracks.map((t) => ({ ...t }));
    q.index = tracks.length ? index : null;
  });
  await settle();
  return wrapper;
}
const rows = (w: Awaited<ReturnType<typeof mountQueue>>) => w.findAll('[data-testid="queue-row"]');
const btn = (w: Awaited<ReturnType<typeof mountQueue>>, text: string | RegExp) =>
  w.findAll("button").find((x) => (typeof text === "string" ? x.text() === text : text.test(x.text())))!;

function dnd(target: Element, type: "dragstart" | "dragover" | "drop" | "dragend" | "dragleave", store: Record<string, string>) {
  const e = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(e, "dataTransfer", {
    value: { effectAllowed: "", dropEffect: "", setData: (k: string, v: string) => (store[k] = v), getData: (k: string) => store[k] ?? "" },
  });
  target.dispatchEvent(e);
  return e;
}

describe("QueueView", () => {
  it("shows every queued track with title and artist, numbered from 1", async () => {
    const w = await mountQueue();
    expect(w.text()).toContain("3 tracks");
    expect(rows(w).map((r) => r.text().replace(/\s+/g, " "))).toEqual([
      expect.stringContaining("1"),
      expect.stringContaining("2"),
      expect.stringContaining("3"),
    ]);
    expect(rows(w)[0].text()).toContain("Alpha");
    expect(rows(w)[0].text()).toContain("AA");
    expect(rows(w)[2].text()).toContain("Gamma");
  });

  it("highlights the row that is playing", async () => {
    const w = await mountQueue([a, b, c], 1);
    expect(rows(w).map((r) => r.attributes("data-current") !== undefined)).toEqual([false, true, false]);
  });

  it("explains an empty queue and disables the bulk actions", async () => {
    const w = await mountQueue([], null);
    expect(w.text()).toContain("Queue is empty");
    expect(btn(w, "Save queue as playlist").attributes("disabled")).toBeDefined();
    expect(btn(w, "Clear").attributes("disabled")).toBeDefined();
  });

  describe("row actions", () => {
    it("disables Move up on the first row and Move down on the last", async () => {
      const w = await mountQueue();
      expect(rows(w)[0].get('button[aria-label="Move up"]').attributes("disabled")).toBeDefined();
      expect(rows(w)[0].get('button[aria-label="Move down"]').attributes("disabled")).toBeUndefined();
      expect(rows(w)[2].get('button[aria-label="Move down"]').attributes("disabled")).toBeDefined();
    });

    it("moves a track down and re-syncs the engine, keeping the playing track current", async () => {
      tauri.on("get_state", makeState({ status: "playing", position_ms: 30_000 }));
      const w = await mountQueue([a, b, c], 0);
      await usePlayerStore().init();
      await rows(w)[0].get('button[aria-label="Move down"]').trigger("click");
      await settle();
      const q = useQueueStore();
      expect(q.tracks.map((t) => t.title)).toEqual(["Beta", "Alpha", "Gamma"]);
      expect(q.index).toBe(1); // Alpha is still the one playing
      expect(tauri.callsTo("queue_play")).toHaveLength(1);
      const seeks = tauri.callsTo("seek_ms") as { ms: number }[];
      expect(seeks).toHaveLength(1);
      expect(seeks[0].ms).toBeGreaterThanOrEqual(30_000); // resumes near where it was
      expect(seeks[0].ms).toBeLessThan(31_000);
    });

    it("removes a track; removing the last one stops playback", async () => {
      const w = await mountQueue([a, b], 0);
      await rows(w)[1].get('button[aria-label="Remove from queue"]').trigger("click");
      await settle();
      expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["Alpha"]);
      await rows(w)[0].get('button[aria-label="Remove from queue"]').trigger("click");
      await settle();
      expect(useQueueStore().tracks).toEqual([]);
      expect(tauri.callsTo("stop")).toHaveLength(1);
    });

    it("keeps the playing track current when an earlier row is removed", async () => {
      const w = await mountQueue([a, b, c], 2);
      await rows(w)[0].get('button[aria-label="Remove from queue"]').trigger("click");
      await settle();
      expect(useQueueStore().index).toBe(1);
      expect(useQueueStore().current?.title).toBe("Gamma");
    });

    it("Clear empties the queue and stops", async () => {
      const w = await mountQueue();
      await btn(w, "Clear").trigger("click");
      await settle();
      expect(useQueueStore().tracks).toEqual([]);
      expect(tauri.callsTo("stop")).toHaveLength(1);
      expect(w.text()).toContain("Queue is empty");
    });
  });

  describe("drag and drop reordering", () => {
    it("drops a row onto another to reorder", async () => {
      const w = await mountQueue();
      const store: Record<string, string> = {};
      dnd(rows(w)[0].element, "dragstart", store);
      dnd(rows(w)[2].element, "dragover", store);
      await settle();
      expect(rows(w)[2].classes().join(" ")).toContain("inset_0_2px_0"); // drop indicator
      dnd(rows(w)[2].element, "drop", store);
      await settle();
      expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["Beta", "Gamma", "Alpha"]);
    });

    it("dims the dragged row and clears the indicator when the drag ends without a drop", async () => {
      const w = await mountQueue();
      const store: Record<string, string> = {};
      dnd(rows(w)[0].element, "dragstart", store);
      await settle();
      expect(rows(w)[0].classes()).toContain("opacity-40");
      dnd(rows(w)[0].element, "dragend", store);
      await settle();
      expect(rows(w)[0].classes()).not.toContain("opacity-40");
      expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["Alpha", "Beta", "Gamma"]);
    });

    it("dropping a row on itself does nothing", async () => {
      const w = await mountQueue();
      const store: Record<string, string> = {};
      dnd(rows(w)[1].element, "dragstart", store);
      dnd(rows(w)[1].element, "drop", store);
      await settle();
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
    });
  });

  describe("save as playlist (dialog instead of window.prompt)", () => {
    it("asks for a name, then saves the queue's track ids", async () => {
      const calls = mockFetch({ "/api/playlists": () => new Response(JSON.stringify({ id: 4, name: "Evening", track_ids: [a.id, b.id, c.id] }), { status: 200 }) });
      const w = await mountQueue();
      await btn(w, "Save queue as playlist").trigger("click");
      await settle();
      expect(dialog()!.textContent).toContain("Save queue as playlist");
      await typeInto(dialog()!.querySelector("input")!, "  Evening ");
      $$("[role=dialog] button").find((x) => x.textContent?.trim() === "Save")!.click();
      await settle();
      expect(bodyOf(calls[0].init)).toEqual({ name: "Evening", from_queue: true, queue_track_ids: [a.id, b.id, c.id] });
      expect(dialog()).toBeNull();
    });

    it("shows the failure and lets the user retry", async () => {
      mockFetch({ "/api/playlists": () => new Response(JSON.stringify({ error: "disk full" }), { status: 500 }) });
      const w = await mountQueue();
      await btn(w, "Save queue as playlist").trigger("click");
      await settle();
      await typeInto(dialog()!.querySelector("input")!, "X");
      $$("[role=dialog] button").find((x) => x.textContent?.trim() === "Save")!.click();
      await settle();
      expect(w.get('[role="alert"]').text()).toBe("disk full");
      expect(btn(w, "Save queue as playlist").attributes("disabled")).toBeUndefined();
    });

    it("does nothing when the dialog is cancelled", async () => {
      const calls = mockFetch({});
      const w = await mountQueue();
      await btn(w, "Save queue as playlist").trigger("click");
      await settle();
      $$("[role=dialog] button").find((x) => x.textContent?.trim() === "Cancel")!.click();
      await settle();
      expect(calls).toHaveLength(0);
    });
  });

  describe("shuffle and repeat", () => {
    it("reflects and toggles them", async () => {
      tauri.on("get_state", makeState({ shuffle: true, repeat: "all" }));
      const w = await mountQueue();
      await usePlayerStore().init();
      await settle();
      const shuffle = w.findAll("button").find((x) => x.attributes("title") === "Shuffle")!;
      const repeat = w.findAll("button").find((x) => x.attributes("title")?.startsWith("Repeat"))!;
      expect(shuffle.attributes("aria-pressed")).toBe("true");
      expect(repeat.attributes("aria-pressed")).toBe("true");
      expect(repeat.text()).toContain("Repeat all");
      await shuffle.trigger("click");
      await repeat.trigger("click");
      expect(tauri.callsTo("set_shuffle")).toEqual([{ on: false }]);
      expect(tauri.callsTo("set_repeat")).toHaveLength(1);
    });
  });
});
