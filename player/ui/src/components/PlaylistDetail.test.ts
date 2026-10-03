import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeTrack, mockFetch } from "../test/fixtures";
import { $$, bodyOf, dialog, mountApp, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useToastsStore } from "../stores/toasts";
import { useViewPrefsStore } from "../stores/viewPrefs";
import PlaylistDetail from "./PlaylistDetail.vue";

const t1 = makeTrack({ title: "One" });
const t2 = makeTrack({ title: "Two" });
const t3 = makeTrack({ title: "Three", missing: true });
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

function routes(playlist = { id: 5, name: "Mix", track_ids: [t1.id, t2.id, t3.id] }) {
  return mockFetch({
    "/api/playlists/5/tracks": (_u: string, init?: RequestInit) => json({ ...playlist, track_ids: (bodyOf(init) as { track_ids: number[] }).track_ids }),
    "/api/playlists/5": () => json(playlist),
    [`/api/tracks/${t1.id}`]: t1,
    [`/api/tracks/${t2.id}`]: t2,
    [`/api/tracks/${t3.id}`]: t3,
    "/api/playlists": [], // every row's TrackMenu loads the list on mount
  });
}

async function mountDetail(playlist?: { id: number; name: string; track_ids: number[] }) {
  const calls = routes(playlist);
  const { wrapper } = mountApp(PlaylistDetail, { id: 5 });
  await settle();
  return { w: wrapper, calls };
}
type W = Awaited<ReturnType<typeof mountDetail>>["w"];
const btn = (w: W, text: string | RegExp) => w.findAll("button").find((b) => (typeof text === "string" ? b.text() === text : text.test(b.text())))!;
const trackRows = (w: W) => w.findAll('[data-playable]');

describe("PlaylistDetail", () => {
  it("loads the playlist and its tracks in order", async () => {
    const { w } = await mountDetail();
    expect(w.get("h2").text()).toBe("Mix");
    expect(w.text()).toContain("3 tracks");
    expect(trackRows(w).map((r) => r.find(".truncate").text())).toEqual(["One", "Two", "Three"]);
  });

  it("shows a loading state and then an error if the playlist cannot be loaded", async () => {
    mockFetch({ "/api/playlists/5": () => json({ error: "playlist 5 not found" }, 404), "/api/playlists": [] });
    const { wrapper } = mountApp(PlaylistDetail, { id: 5 });
    await settle();
    expect(wrapper.get('[role="alert"]').text()).toBe("playlist 5 not found");
  });

  it("explains an empty playlist", async () => {
    const { w } = await mountDetail({ id: 5, name: "Empty", track_ids: [] });
    expect(w.text()).toContain("This playlist is empty.");
  });

  it("goes back to the list", async () => {
    const { w } = await mountDetail();
    await btn(w, /Playlists/).trigger("click");
    expect(useNavStore().view.name).toBe("playlists");
  });

  describe("playback", () => {
    it("Play queues only the playable tracks, from the start", async () => {
      const { w } = await mountDetail();
      await btn(w, /Play$/).trigger("click");
      await settle();
      const call = tauri.callsTo("queue_play")[0] as { tracks: { id: number }[]; index: number };
      expect(call.tracks.map((t) => t.id)).toEqual([t1.id, t2.id]);
      expect(call.index).toBe(0);
    });

    it("double-clicking a row plays from there; an unplayable row does not play", async () => {
      const { w } = await mountDetail();
      await trackRows(w)[1].trigger("dblclick");
      await settle();
      expect((tauri.callsTo("queue_play")[0] as { index: number }).index).toBe(1);
      await trackRows(w)[2].trigger("dblclick");
      await settle();
      expect(tauri.callsTo("queue_play")).toHaveLength(1);
    });

    it("Add to queue appends the playable tracks and confirms", async () => {
      const { w } = await mountDetail();
      await btn(w, /Add to queue/).trigger("click");
      await settle();
      expect((tauri.callsTo("queue_append")[0] as { tracks: unknown[] }).tracks).toHaveLength(2);
      expect(useToastsStore().toasts.at(-1)?.title).toBe("Added 2 tracks to queue");
    });
  });

  describe("editing", () => {
    it("renames inline", async () => {
      const { w, calls } = await mountDetail();
      await w.get('button[aria-label="Rename playlist"]').trigger("click");
      const input = w.get('input[aria-label="Playlist name"]');
      expect((input.element as HTMLInputElement).value).toBe("Mix");
      await typeInto(input.element as HTMLInputElement, "Better mix");
      await input.trigger("keydown", { key: "Enter" });
      await settle();
      const patch = calls.find((c) => c.init?.method === "PATCH");
      // (the stub returns the stored name; what matters is the request)
      expect(bodyOf(patch!.init)).toEqual({ name: "Better mix" });
    });

    it("ignores an unchanged or blank name", async () => {
      const { w, calls } = await mountDetail();
      await w.get('button[aria-label="Rename playlist"]').trigger("click");
      await w.get('input[aria-label="Playlist name"]').trigger("keydown", { key: "Enter" });
      await settle();
      await w.get('button[aria-label="Rename playlist"]').trigger("click");
      await typeInto(w.get('input[aria-label="Playlist name"]').element as HTMLInputElement, "   ");
      await w.get('input[aria-label="Playlist name"]').trigger("keydown", { key: "Enter" });
      await settle();
      expect(calls.some((c) => c.init?.method === "PATCH")).toBe(false);
    });

    it("moves a track down by saving the new order", async () => {
      const { w, calls } = await mountDetail();
      await trackRows(w)[0].get('button[aria-label="Move down"]').trigger("click");
      await settle();
      const put = calls.find((c) => c.init?.method === "PUT")!;
      expect(bodyOf(put.init)).toMatchObject({ track_ids: [t2.id, t1.id, t3.id], mode: "replace" });
    });

    it("disables Move up on the first row and Move down on the last", async () => {
      const { w } = await mountDetail();
      expect(trackRows(w)[0].get('button[aria-label="Move up"]').attributes("disabled")).toBeDefined();
      expect(trackRows(w)[2].get('button[aria-label="Move down"]').attributes("disabled")).toBeDefined();
    });

    it("removes a track from the playlist", async () => {
      const { w, calls } = await mountDetail();
      await trackRows(w)[1].get('button[aria-label="Remove from playlist"]').trigger("click");
      await settle();
      expect(bodyOf(calls.find((c) => c.init?.method === "PUT")!.init)).toMatchObject({ track_ids: [t1.id, t3.id] });
    });

    it("toasts and restores the order when saving fails", async () => {
      routes();
      mockFetch({
        "/api/playlists/5/tracks": () => json({ error: "locked" }, 500),
        "/api/playlists/5": () => json({ id: 5, name: "Mix", track_ids: [t1.id, t2.id, t3.id] }),
        [`/api/tracks/${t1.id}`]: t1, [`/api/tracks/${t2.id}`]: t2, [`/api/tracks/${t3.id}`]: t3,
        "/api/playlists": [],
      });
      const { wrapper } = mountApp(PlaylistDetail, { id: 5 });
      await settle();
      await trackRows(wrapper)[0].get('button[aria-label="Move down"]').trigger("click");
      await settle();
      expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "error", title: "Reorder failed" });
      expect(usePlaylistsStore().detail?.playlist.track_ids).toEqual([t1.id, t2.id, t3.id]);
    });
  });

  describe("delete (confirm dialog)", () => {
    it("asks first, deletes on confirm, and returns to the list", async () => {
      const { w, calls } = await mountDetail();
      await btn(w, "Delete").trigger("click");
      await settle();
      expect(dialog()!.textContent).toContain("Delete this playlist?");
      expect(calls.some((c) => c.init?.method === "DELETE")).toBe(false);
      $$("[role=dialog] button").find((b) => b.textContent?.trim() === "Delete")!.click();
      await settle();
      expect(calls.some((c) => c.init?.method === "DELETE" && c.url.endsWith("/api/playlists/5"))).toBe(true);
      expect(useNavStore().view.name).toBe("playlists");
    });

    it("does nothing when cancelled", async () => {
      const { w, calls } = await mountDetail();
      await btn(w, "Delete").trigger("click");
      await settle();
      $$("[role=dialog] button").find((b) => b.textContent?.trim() === "Cancel")!.click();
      await settle();
      expect(calls.some((c) => c.init?.method === "DELETE")).toBe(false);
      expect(useNavStore().view.name).toBe("albums");
    });
  });

  describe("list or grid, and dragging to reorder", () => {
    const ROW = 52;
    const pointer = (type: string, x: number, y: number) =>
      new PointerEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: y, pointerType: "mouse" });
    const rows = (w: W) => w.findAll('[data-testid="playlist-row"]');
    // The list and its scroller sit at the window's top-left, 800 × 600.
    beforeEach(() => {
      vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(
        { left: 0, top: 0, right: 800, bottom: 600, width: 800, height: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect,
      );
    });
    afterEach(() => vi.restoreAllMocks());

    async function drag(w: W, from: number, y: number, release = true) {
      const start = from * ROW + ROW / 2;
      rows(w)[from].element.dispatchEvent(pointer("pointerdown", 40, start));
      window.dispatchEvent(pointer("pointermove", 40, start + 8));
      window.dispatchEvent(pointer("pointermove", 40, y));
      await settle();
      if (release) {
        window.dispatchEvent(pointer("pointerup", 40, y));
        await settle();
      }
    }

    it("shows the tracks as a grid of cards, and remembers it", async () => {
      const { w } = await mountDetail();
      expect(w.findAll('[data-testid="track-card"]')).toHaveLength(0);
      await w.get('button[aria-label="Grid"]').trigger("click");
      await settle();
      expect(useViewPrefsStore().prefs.playlistTracksLayout).toBe("grid");
      expect(w.findAll('[data-testid="track-card"]').length).toBe(3);
      expect(w.find('[data-testid="playlist-drag-hint"]').exists()).toBe(false);
    });

    it("while dragging, the row stays dimmed in a dashed outline and the gap it will land in is marked", async () => {
      const { w } = await mountDetail();
      await drag(w, 0, 2 * ROW + 4, false);
      expect(rows(w)[0].classes()).toContain("opacity-40");
      expect(w.find('[data-testid="drag-outline"]').exists()).toBe(true);
      expect(w.get('[data-testid="drop-slot"]').attributes("data-slot")).toBe("2");
      expect(document.body.querySelector('[data-testid="drag-ghost"]')?.textContent).toContain("One");
      window.dispatchEvent(pointer("pointerup", 40, 2 * ROW + 4));
      await settle();
    });

    it("dropping in the gap saves the new order", async () => {
      const { w, calls } = await mountDetail();
      await drag(w, 0, 3 * ROW - 2); // below the last row
      const put = calls.filter((c) => c.url.endsWith("/api/playlists/5/tracks")).at(-1);
      expect((bodyOf(put?.init) as { track_ids: number[] }).track_ids).toEqual([t2.id, t3.id, t1.id]);
      expect(w.find('[data-testid="drag-outline"]').exists()).toBe(false);
      expect(document.body.querySelector('[data-testid="drag-ghost"]')).toBeNull();
    });

    it("Escape cancels, and the row's buttons never start a drag", async () => {
      const { w, calls } = await mountDetail();
      await drag(w, 0, 3 * ROW - 2, false);
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      window.dispatchEvent(pointer("pointerup", 40, 3 * ROW - 2));
      await settle();
      rows(w)[1].get('button[aria-label="Remove from playlist"]').element.dispatchEvent(pointer("pointerdown", 700, 70));
      window.dispatchEvent(pointer("pointermove", 700, 150));
      await settle();
      expect(w.find('[data-testid="drag-outline"]').exists()).toBe(false);
      window.dispatchEvent(pointer("pointerup", 700, 150));
      await settle();
      expect(calls.some((c) => c.url.endsWith("/api/playlists/5/tracks"))).toBe(false);
    });
  });
});
