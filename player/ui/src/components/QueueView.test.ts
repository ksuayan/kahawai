import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeAlbum, makeState, makeTrack, mockFetch } from "../test/fixtures";
import { $$, bodyOf, dialog, mountApp, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "../stores/player";
import { useLibraryStore } from "../stores/library";
import { useQueueStore } from "../stores/queue";
import { useViewPrefsStore } from "../stores/viewPrefs";
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

/** Row height in the Queue's list (the drop geometry is computed from it). */
const ROW = 52;
const pointer = (type: string, x: number, y: number) =>
  new PointerEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: y, pointerType: "mouse" });

/** Press on row `from`, move to `y` (px from the top of the list), and optionally let go. */
async function drag(w: Awaited<ReturnType<typeof mountQueue>>, from: number, y: number, release = true) {
  const start = from * ROW + ROW / 2;
  rows(w)[from].element.dispatchEvent(pointer("pointerdown", 40, start));
  window.dispatchEvent(pointer("pointermove", 40, start + 8)); // past the threshold: the drag begins
  window.dispatchEvent(pointer("pointermove", 40, y));
  await settle();
  if (release) {
    window.dispatchEvent(pointer("pointerup", 40, y));
    await settle();
  }
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

  describe("track details (like the album track list)", () => {
    const hires = makeTrack({ title: "Smoke Signals", artist: "Phoebe Bridgers", album: "Stranger in the Alps", format: "m4a", bit_depth: 24, sample_rate: 96000, bitrate: 2647, channels: 2, album_id: 42 });
    const mqa = makeTrack({ title: "Brahms Lullaby", format: "flac", bit_depth: 24, sample_rate: 48000, bitrate: 1400, mqa: true, original_sample_rate: 48000 });

    it("shows each track's format and quality badge with the full detail on hover", async () => {
      const w = await mountQueue([hires, a]);
      const badges = w.findAll('[data-testid="format-badge"]');
      expect(badges.map((b) => b.text())).toEqual(["M4A · 24/96k", "FLAC · 16/44.1k"]);
      expect(badges[0].attributes("title")).toBe("M4A · 24-bit / 96 kHz · 2647 kbps · 2 ch");
    });

    it("shows artist and album on the second line", async () => {
      const w = await mountQueue([hires]);
      expect(w.get('[data-testid="detail-line"]').text()).toBe("Phoebe Bridgers — Stranger in the Alps");
    });

    it("badges MQA tracks with the master rate, and only those", async () => {
      const w = await mountQueue([mqa, hires]);
      expect(rows(w)[0].get('[data-testid="mqa-badge"]').text()).toBe("MQA · 48k");
      expect(rows(w)[0].get('[data-testid="mqa-badge"]').attributes("title")).toContain("plays as ordinary FLAC");
      expect(rows(w)[1].find('[data-testid="mqa-badge"]').exists()).toBe(false);
    });

    it("shows the album cover for a track (not a blank placeholder), when the library has it", async () => {
      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
      const { wrapper } = mountApp(QueueView, {}, {}, () => {
        const q = useQueueStore();
        q.tracks = [{ ...hires }, { ...a }];
        q.index = 0;
        useLibraryStore().albums = [makeAlbum({ id: 42, artwork_hash: "cafe42" })];
      });
      await settle();
      const covers = wrapper.findAll('[data-testid="queue-row"]').map((r) => r.find("img").exists() && r.get("img").attributes("src"));
      expect(covers).toEqual(["artwork://localhost/cafe42", false]);
    });

    it("keeps the badges next to the duration, before the row buttons", async () => {
      const w = await mountQueue([hires]);
      const kids = Array.from(rows(w)[0].element.children).map((c) => c.getAttribute("data-testid") ?? c.tagName);
      const iFormat = kids.indexOf("format-badge");
      expect(iFormat).toBeGreaterThan(-1);
      expect(rows(w)[0].text()).toContain("3:20");
      expect(kids.indexOf("format-badge")).toBeLessThan(kids.length - 2);
    });
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

  describe("playing from the queue (double-click)", () => {
    it("double-clicking a row plays the queue from that track", async () => {
      const w = await mountQueue([a, b, c], 0);
      await rows(w)[2].trigger("dblclick");
      await settle();
      const call = tauri.callsTo("queue_play")[0] as { tracks: { id: number }[]; index: number };
      expect(call.index).toBe(2);
      expect(call.tracks.map((t) => t.id)).toEqual([a.id, b.id, c.id]); // the queue is kept whole
      expect(useQueueStore().index).toBe(2);
      expect(useQueueStore().current?.title).toBe("Gamma");
    });

    it("Enter on a focused row plays it too (keyboard access)", async () => {
      const w = await mountQueue();
      await rows(w)[1].trigger("keydown", { key: "Enter" });
      await settle();
      expect((tauri.callsTo("queue_play")[0] as { index: number }).index).toBe(1);
      expect(rows(w)[1].attributes("tabindex")).toBe("0");
    });

    it("a track that cannot play (missing file) is dimmed and ignores double-click", async () => {
      const bad = makeTrack({ title: "Gone", missing: true });
      const w = await mountQueue([a, bad], 0);
      expect(rows(w)[1].classes()).toContain("opacity-45");
      expect(rows(w)[1].attributes("title")).toMatch(/missing/i);
      await rows(w)[1].trigger("dblclick");
      await settle();
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
    });

    it("tells the user how to play a row", async () => {
      const w = await mountQueue();
      expect(rows(w)[0].attributes("title")).toContain("Double-click to play");
    });

    it("only the drag handle shows the grab cursor, not the whole row (regression: hand cursor everywhere)", async () => {
      const w = await mountQueue();
      expect(rows(w)[0].classes()).toContain("cursor-default");
      expect(rows(w)[0].classes()).not.toContain("cursor-grab");
      expect(rows(w)[0].find("svg.cursor-grab").exists()).toBe(true);
    });

    it("dragging still works alongside double-click", async () => {
      const w = await mountQueue();
      expect(rows(w)[0].attributes("data-reorderable")).toBe("true");
    });
  });

  describe("row actions", () => {
    it("disables Move up on the first row and Move down on the last", async () => {
      const w = await mountQueue();
      expect(rows(w)[0].get('button[aria-label="Move up"]').attributes("disabled")).toBeDefined();
      expect(rows(w)[0].get('button[aria-label="Move down"]').attributes("disabled")).toBeUndefined();
      expect(rows(w)[2].get('button[aria-label="Move down"]').attributes("disabled")).toBeDefined();
    });

    it("moves a track down in place, keeping the playing track current and playing", async () => {
      tauri.on("get_state", makeState({ status: "playing", position_ms: 30_000 }));
      const w = await mountQueue([a, b, c], 0);
      await usePlayerStore().init();
      await rows(w)[0].get('button[aria-label="Move down"]').trigger("click");
      await settle();
      const q = useQueueStore();
      expect(q.tracks.map((t) => t.title)).toEqual(["Beta", "Alpha", "Gamma"]);
      expect(q.index).toBe(1); // Alpha is still the one playing
      expect(tauri.callsTo("queue_move")).toEqual([{ from: 0, to: 1 }]);
      expect(tauri.callsTo("queue_play")).toHaveLength(0); // nothing restarts
      expect(tauri.callsTo("seek_ms")).toHaveLength(0);
    });

    it("removes a track in place; removing the last one leaves the core to stop", async () => {
      const w = await mountQueue([a, b], 0);
      await rows(w)[1].get('button[aria-label="Remove from queue"]').trigger("click");
      await settle();
      expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["Alpha"]);
      await rows(w)[0].get('button[aria-label="Remove from queue"]').trigger("click");
      await settle();
      expect(useQueueStore().tracks).toEqual([]);
      expect(tauri.callsTo("queue_remove")).toEqual([{ index: 1 }, { index: 0 }]);
      expect(tauri.callsTo("queue_play")).toHaveLength(0);
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
      expect(tauri.callsTo("queue_play").at(-1)).toMatchObject({ tracks: [] });
      expect(w.text()).toContain("Queue is empty");
    });
  });

  describe("drag and drop reordering", () => {
    // The list and its scroller sit at the window's top-left, 800 × 600.
    beforeEach(() => {
      vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(
        { left: 0, top: 0, right: 800, bottom: 600, width: 800, height: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect,
      );
    });
    afterEach(() => vi.restoreAllMocks());

    it("while dragging: the row stays dimmed in a dashed outline, a copy follows the pointer, and the gap it will land in is marked", async () => {
      const w = await mountQueue();
      await drag(w, 0, 2 * ROW + 4, false); // between Beta and Gamma
      expect(rows(w)[0].classes()).toContain("opacity-40");
      const outline = w.get('[data-testid="drag-outline"]');
      expect(outline.classes().join(" ")).toContain("border-dashed");
      expect(outline.attributes("style")).toContain(`top: ${0 * ROW + 1}px`);
      expect(w.get('[data-testid="drop-slot"]').attributes("data-slot")).toBe("2");
      // The rows beside the gap step apart.
      expect(rows(w)[1].attributes("style")).toContain("translateY(-4px)");
      expect(rows(w)[2].attributes("style")).toContain("translateY(4px)");
      expect(document.body.querySelector('[data-testid="drag-ghost"]')?.textContent).toContain("Alpha");
      window.dispatchEvent(pointer("pointerup", 40, 2 * ROW + 4));
      await settle();
    });

    it("dropping in the gap moves the row there, in place", async () => {
      const w = await mountQueue();
      await drag(w, 0, 3 * ROW - 2); // below Gamma
      expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["Beta", "Gamma", "Alpha"]);
      expect(tauri.callsTo("queue_move")).toEqual([{ from: 0, to: 2 }]);
      await drag(w, 2, 2); // Alpha back to the top
      expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["Alpha", "Beta", "Gamma"]);
      // Everything is cleared afterwards.
      expect(w.find('[data-testid="drag-outline"]').exists()).toBe(false);
      expect(w.find('[data-testid="drop-slot"]').exists()).toBe(false);
      expect(document.body.querySelector('[data-testid="drag-ghost"]')).toBeNull();
    });

    it("offers no slot beside the row itself, and dropping there does nothing", async () => {
      const w = await mountQueue();
      await drag(w, 1, ROW + 4, false); // just above Beta, its own place
      expect(w.find('[data-testid="drop-slot"]').exists()).toBe(false);
      window.dispatchEvent(pointer("pointerup", 40, ROW + 4));
      await settle();
      expect(tauri.callsTo("queue_move")).toHaveLength(0);
    });

    it("Escape cancels: nothing moves", async () => {
      const w = await mountQueue();
      await drag(w, 0, 3 * ROW - 2, false);
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await settle();
      expect(w.find('[data-testid="drag-outline"]').exists()).toBe(false);
      window.dispatchEvent(pointer("pointerup", 40, 3 * ROW - 2));
      await settle();
      expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["Alpha", "Beta", "Gamma"]);
    });

    it("a press without moving is a click, not a drag; the row's buttons never start one", async () => {
      const w = await mountQueue();
      rows(w)[0].element.dispatchEvent(pointer("pointerdown", 40, 20));
      window.dispatchEvent(pointer("pointerup", 40, 21));
      await settle();
      expect(w.find('[data-testid="drag-outline"]').exists()).toBe(false);
      rows(w)[0].get('button[aria-label="Remove from queue"]').element.dispatchEvent(pointer("pointerdown", 700, 20));
      window.dispatchEvent(pointer("pointermove", 700, 140));
      await settle();
      expect(w.find('[data-testid="drag-outline"]').exists()).toBe(false);
      window.dispatchEvent(pointer("pointerup", 700, 140));
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

describe("QueueView: sort and grid", () => {
  const x = makeTrack({ title: "Xi", artist: "Zed", album: "Z", year: 1990 });
  const y = makeTrack({ title: "Ypsilon", artist: "Abe", album: "A", year: 2001 });
  const setView = (sort: string, layout = "list") => () => {
    useViewPrefsStore().prefs.queueSort = sort as never;
    useViewPrefsStore().prefs.queueLayout = layout as never;
  };

  it("sorted, it shows another order but keeps the play order and turns rearranging off", async () => {
    const { wrapper: w } = mountApp(QueueView, {}, {}, () => {
      useQueueStore().tracks = [x, y].map((t) => ({ ...t }));
      useQueueStore().index = 0;
      setView("artist-asc")();
    });
    await settle();
    const r = w.findAll('[data-testid="queue-row"]');
    expect(r.map((row) => row.text().includes("Ypsilon"))).toEqual([true, false]); // Abe before Zed
    expect(r[0].text()).toContain("2"); // its real queue position
    expect(w.get('[data-testid="queue-sort-hint"]').text()).toContain("play order is unchanged");
    expect(r[0].attributes("data-reorderable")).toBe("false");
    // A press-and-move on a sorted row starts no drag.
    r[0].element.dispatchEvent(pointer("pointerdown", 40, 20));
    window.dispatchEvent(pointer("pointermove", 40, 90));
    await settle();
    expect(w.find('[data-testid="drag-outline"]').exists()).toBe(false);
    window.dispatchEvent(pointer("pointerup", 40, 90));
    expect(r[0].findAll("button").find((b) => b.attributes("aria-label") === "Move down")!.attributes("disabled")).toBeDefined();

    await r[0].trigger("dblclick");
    await settle();
    const call = tauri.callsTo("queue_play")[0] as { tracks: { id: number }[]; index: number };
    expect(call.index).toBe(1); // Ypsilon's place in the real queue
    expect(call.tracks.map((t) => t.id)).toEqual([x.id, y.id]);
  });

  it("the grid shows every entry, a track queued twice included, with its position", async () => {
    const { wrapper: w } = mountApp(QueueView, {}, {}, () => {
      useQueueStore().tracks = [x, y, x].map((t) => ({ ...t }));
      useQueueStore().index = 2;
      setView("default", "grid")();
    });
    await settle();
    const cards = w.findAll('[data-testid="track-card"]');
    expect(cards).toHaveLength(3);
    expect(cards.map((c) => c.text().slice(0, 1))).toEqual(["1", "2", "3"]);
    expect(cards[2].attributes("data-current")).toBe("true");
    await cards[1].get("button").trigger("click");
    await settle();
    expect((tauri.callsTo("queue_play")[0] as { index: number }).index).toBe(1);
  });
});
