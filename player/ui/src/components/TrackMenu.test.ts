import { describe, expect, it } from "vitest";
import { makeTrack, mockFetch } from "../test/fixtures";
import { $$, bodyOf, dialog, key, menuItems, mountApp, openMenu, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import TrackMenu from "./TrackMenu.vue";

const item = (label: string | RegExp) =>
  menuItems().find((e) => (typeof label === "string" ? e.textContent?.includes(label) : label.test(e.textContent ?? "")))!;

async function setup(props: Record<string, unknown> = { track: makeTrack({ title: "Grenade" }) }, playlists = [{ id: 1, name: "Road trip", track_ids: [1, 2] }, { id: 2, name: "Focus", track_ids: [] }]) {
  const { wrapper } = mountApp(TrackMenu, props);
  const store = usePlaylistsStore();
  store.items = playlists;
  store.loaded = true;
  await settle();
  const trigger = wrapper.get("button[aria-label^='Actions']").element as HTMLElement;
  return { wrapper, trigger, open: () => openMenu(trigger) };
}

async function openSubmenu() {
  const sub = item("Add to playlist");
  sub.dispatchEvent(new PointerEvent("pointermove", { pointerType: "mouse", bubbles: true }));
  key(sub, "ArrowRight");
  await settle();
}

describe("TrackMenu", () => {
  it("names the trigger after the track", async () => {
    const { trigger } = await setup();
    expect(trigger.getAttribute("aria-label")).toBe("Actions for Grenade");
    const { trigger: album } = await setup({ tracks: [makeTrack()], albumId: 3 });
    expect(album.getAttribute("aria-label")).toBe("Actions");
  });

  it("opens with the queue and playlist actions", async () => {
    const { open } = await setup();
    expect(menuItems()).toHaveLength(0);
    await open();
    expect(menuItems().map((e) => e.textContent?.replace(/[^\w ]/g, "").trim())).toEqual(["Play next", "Add to queue", "Add to playlist"]);
  });

  it("Play next inserts after the current track and confirms with a toast", async () => {
    const track = makeTrack({ title: "Grenade" });
    const { open } = await setup({ track });
    await open();
    item("Play next").click();
    await settle();
    expect(tauri.callsTo("queue_insert_next")).toHaveLength(1);
    expect((tauri.callsTo("queue_insert_next")[0] as { tracks: { id: number }[] }).tracks[0].id).toBe(track.id);
    expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "success", title: "Playing next: 1 track" });
    expect(menuItems()).toHaveLength(0); // the menu closes
  });

  it("Add to queue appends and confirms; the local queue shows it", async () => {
    const track = makeTrack();
    const { open } = await setup({ track });
    await open();
    item("Add to queue").click();
    await settle();
    expect(tauri.callsTo("queue_append")).toHaveLength(1);
    expect(useQueueStore().tracks.map((t) => t.id)).toContain(track.id);
    expect(useToastsStore().toasts.at(-1)?.title).toBe("Added 1 track to queue");
  });

  it("rolls back the queue and reports an error when the engine refuses", async () => {
    tauri.on("queue_append", () => {
      throw new Error("engine down");
    });
    const { open } = await setup();
    const queue = useQueueStore();
    await open();
    item("Add to queue").click();
    await settle();
    expect(queue.tracks).toHaveLength(0);
    expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "error", title: "Add to queue failed" });
  });

  it("on an album, acts on the playable tracks only", async () => {
    const good = makeTrack();
    const missing = makeTrack({ missing: true });
    const { open } = await setup({ tracks: [good, missing], albumId: 9 });
    await open();
    item("Add to queue").click();
    await settle();
    expect((tauri.callsTo("queue_append")[0] as { tracks: { id: number }[] }).tracks.map((t) => t.id)).toEqual([good.id]);
    expect(useToastsStore().toasts.at(-1)?.title).toBe("Added 1 track to queue");
  });

  it("disables the actions when there is nothing playable", async () => {
    const { open } = await setup({ tracks: [makeTrack({ missing: true })], albumId: 9 });
    await open();
    for (const label of ["Play next", "Add to queue", "Add to playlist"]) {
      expect(item(label).hasAttribute("data-disabled")).toBe(true);
    }
  });

  it("adds a track to an existing playlist from the submenu", async () => {
    const track = makeTrack();
    const calls = mockFetch({ "/api/playlists/2/tracks": (_u: string, init?: RequestInit) => new Response(JSON.stringify({ id: 2, name: "Focus", track_ids: [track.id] }), { status: 200 }) });
    const { open } = await setup({ track });
    await open();
    await openSubmenu();
    const labels = menuItems().map((e) => e.textContent?.replace(/\s+/g, " ").trim());
    expect(labels).toEqual(expect.arrayContaining(["Road trip (2)", "Focus (0)"]));
    item("Focus").click();
    await settle();
    const put = calls.find((c) => c.init?.method === "PUT")!;
    expect(put.url).toContain("/api/playlists/2/tracks");
    expect(bodyOf(put.init)).toMatchObject({ track_ids: [track.id], mode: "append" });
  });

  it("adds a whole album to a playlist by album id", async () => {
    const calls = mockFetch({ "/api/playlists/1/tracks": () => new Response(JSON.stringify({ id: 1, name: "Road trip", track_ids: [] }), { status: 200 }) });
    const { open } = await setup({ tracks: [makeTrack()], albumId: 5 });
    await open();
    await openSubmenu();
    item("Road trip").click();
    await settle();
    expect(bodyOf(calls.find((c) => c.init?.method === "PUT")!.init)).toMatchObject({ album_ids: [5], mode: "append" });
  });

  it("creates a new playlist through a dialog (not window.prompt) and adds the track to it", async () => {
    const track = makeTrack();
    const calls = mockFetch({
      "/api/playlists/7/tracks": () => new Response(JSON.stringify({ id: 7, name: "Late night", track_ids: [track.id] }), { status: 200 }),
      "/api/playlists": () => new Response(JSON.stringify({ id: 7, name: "Late night", track_ids: [] }), { status: 200 }),
    });
    const { open } = await setup({ track });
    await open();
    await openSubmenu();
    item("New playlist").click();
    await settle();
    expect(dialog()).not.toBeNull();
    await typeInto(dialog()!.querySelector("input")!, "  Late night ");
    $$("[role=dialog] button").find((b) => b.textContent?.includes("Create and add"))!.click();
    await settle();
    const post = calls.find((c) => c.init?.method === "POST")!;
    expect(bodyOf(post.init)).toMatchObject({ name: "Late night" });
    const put = calls.find((c) => c.init?.method === "PUT")!;
    expect(put.url).toContain("/api/playlists/7/tracks");
    expect(usePlaylistsStore().items.some((p) => p.name === "Late night")).toBe(true);
  });

  it("reports a failure to create the playlist", async () => {
    mockFetch({ "/api/playlists": () => new Response(JSON.stringify({ error: "name taken" }), { status: 409 }) });
    const { open } = await setup();
    await open();
    await openSubmenu();
    item("New playlist").click();
    await settle();
    await typeInto(dialog()!.querySelector("input")!, "Dup");
    $$("[role=dialog] button").find((b) => b.textContent?.includes("Create and add"))!.click();
    await settle();
    expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "error", title: "Could not create playlist", detail: "name taken" });
  });

  it("offers ISO extraction only for SACD ISO files that exist", async () => {
    const { open } = await setup({ track: makeTrack({ format: "sacd_iso", decodable: false }) });
    await open();
    expect(item("Extract to DSF")).toBeDefined();
  });

  it.each([
    ["a regular FLAC", { format: "flac" }],
    ["a SACD ISO whose file is missing", { format: "sacd_iso", missing: true }],
  ])("does not offer ISO extraction for %s", async (_name, over) => {
    const { open } = await setup({ track: makeTrack(over as never) });
    await open();
    expect(menuItems().length).toBeGreaterThan(0);
    expect(menuItems().some((e) => e.textContent?.includes("Extract"))).toBe(false);
  });

  it("starts the extraction job", async () => {
    const job = { id: "job-1", kind: "extract_iso", label: "x", status: "queued", progress: 0, message: null, payload: null };
    const calls = mockFetch({
      "/api/jobs": (_u: string, init?: RequestInit) =>
        new Response(JSON.stringify(init?.method === "POST" ? job : [job]), { status: 200 }),
    });
    const track = makeTrack({ format: "sacd_iso", decodable: false, path: "/m/a.iso" });
    const { open } = await setup({ track });
    await open();
    item("Extract to DSF").click();
    await settle();
    const post = calls.find((c) => c.init?.method === "POST")!;
    expect(bodyOf(post.init)).toMatchObject({ kind: "extract_iso", path: "/m/a.iso" });
  });

  it("swallows a double-click so it does not also play the row underneath", async () => {
    let bubbled = false;
    const { wrapper } = await setup();
    wrapper.element.parentElement!.addEventListener("dblclick", () => (bubbled = true));
    wrapper.element.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(bubbled).toBe(false);
  });
});
