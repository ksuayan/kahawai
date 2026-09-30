import { mount } from "@vue/test-utils";
import type { Pinia } from "pinia";
import { describe, expect, it } from "vitest";
import { makeAlbum, makeTrack, mockFetch } from "../test/fixtures";
import { menuItems, mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useLibraryActions } from "../lib/libraryActions";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useOverlaysStore } from "../stores/overlays";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import AlbumCard from "./AlbumCard.vue";
import InfoDialog from "./InfoDialog.vue";
import TrackRow from "./TrackRow.vue";

const labels = () => menuItems().map((e) => e.textContent?.trim());
const item = (label: string) => menuItems().find((e) => e.textContent?.trim() === label)!;

/** Right-click, as the webview delivers it. */
async function rightClick(el: Element): Promise<void> {
  el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
  await settle();
}

/** The Info dialog, sharing the stores of the menu that opens it. */
const mountInfo = (pinia: Pinia) => mount(InfoDialog, { attachTo: document.body, global: { plugins: [pinia] } });

const inApp = () => ((window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {});

describe("right-click menu on a track", () => {
  const track = makeTrack({ title: "Fortnight", artist: "Taylor Swift feat. Post Malone", album_id: 55, genre: "Pop" });

  function mountTrack() {
    mockFetch({ "/api/playlists": [] });
    return mountApp(TrackRow, { track }, {}, () => {
      useLibraryStore().artists = [{ id: 3, name: "Taylor Swift" }];
      usePlaylistsStore().loaded = true;
    });
  }

  it("offers the item actions", async () => {
    const { wrapper: w } = mountTrack();
    await rightClick(w.get("[data-playable]").element);
    expect(labels()).toEqual(["Play", "Go to Artist", "Go to Album", "Add to Queue", "Add to Playlist", "Info"]);
  });

  it("plays through the view, and goes to the lead artist of a featured credit", async () => {
    const { wrapper: w } = mountTrack();
    await rightClick(w.get("[data-playable]").element);
    item("Play").click();
    await settle();
    expect(w.emitted("play")).toEqual([[track]]);
    await rightClick(w.get("[data-playable]").element);
    item("Go to Artist").click();
    expect(useNavStore().view).toEqual({ name: "artist", id: 3 });
    await rightClick(w.get("[data-playable]").element);
    item("Go to Album").click();
    expect(useNavStore().view).toEqual({ name: "album", id: 55 });
  });

  it("Info opens the dialog with the details", async () => {
    const { wrapper: w, pinia } = mountTrack();
    const info = mountInfo(pinia);
    await rightClick(w.get("[data-playable]").element);
    item("Info").click();
    await settle();
    expect(useOverlaysStore().info).toEqual({ kind: "track", track });
    const dlg = document.body.querySelector('[data-testid="info-dialog"]')!;
    expect(dlg.textContent).toContain("Fortnight");
    expect(dlg.querySelector('[data-testid="info-Genre"]')!.textContent?.trim()).toBe("Pop");
    expect(dlg.querySelector('[data-testid="info-File"]')!.textContent?.trim()).toBe(track.path);
    expect(dlg.querySelector('[data-testid="info-Content hash"]')!.textContent).toMatch(/BLAKE3|Not hashed/);
    info.unmount();
  });
});

describe("right-click menu on an album", () => {
  it("plays the album's tracks, and Info lists the folders they live in", async () => {
    inApp();
    const album = makeAlbum({ id: 55, title: "THE TORTURED POETS DEPARTMENT", artist: "Taylor Swift" });
    const a = makeTrack({ id: 1, album_id: 55, track_no: 1, path: "/Volumes/NetMusic/TS/1.flac" });
    const b = makeTrack({ id: 2, album_id: 55, track_no: 1, path: "/Volumes/DATA/_Dev-Media/TS/1.flac" });
    mockFetch({ "/api/albums/55": { album: { ...album, track_ids: [1, 2] }, tracks: [a, b] }, "/api/playlists": [] });
    const { wrapper, pinia } = mountApp(AlbumCard, { album, subtitle: "" }, {}, () => {
      useLibraryStore().albums = [album];
      usePlaylistsStore().loaded = true;
    });
    const info = mountInfo(pinia);
    await rightClick(wrapper.get("button").element);
    item("Play").click();
    await settle();
    expect(useQueueStore().tracks.map((t) => t.id)).toEqual([1, 2]);
    expect(tauri.callsTo("queue_play")).toHaveLength(1);

    await rightClick(wrapper.get("button").element);
    item("Info").click();
    await settle();
    await settle();
    const folders = document.body.querySelector('[data-testid="info-Folders"]')!.textContent!;
    expect(folders).toContain("/Volumes/NetMusic/TS");
    expect(folders).toContain("/Volumes/DATA/_Dev-Media/TS");
    info.unmount();
  });
});

describe("adding without duplicates", () => {
  it("the queue gets only what it doesn't have; nothing new is just a notice", async () => {
    inApp();
    mountApp(TrackRow, { track: makeTrack() });
    const [x, y, z] = [makeTrack(), makeTrack(), makeTrack()];
    useQueueStore().tracks = [{ ...x }];
    const actions = useLibraryActions();
    await actions.addToQueue([x, y, y, z]);
    expect(useQueueStore().tracks.map((t) => t.id)).toEqual([x.id, y.id, z.id]);
    expect(useToastsStore().toasts.at(-1)).toMatchObject({ title: "Added 2 tracks to queue", detail: "1 track already in the queue" });

    const before = tauri.callsTo("queue_append").length;
    await actions.addToQueue([y]);
    expect(tauri.callsTo("queue_append")).toHaveLength(before);
    expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "info", title: "Already in the queue" });
  });

  it("a playlist gets only what it doesn't have", async () => {
    const [x, y] = [makeTrack(), makeTrack()];
    const calls = mockFetch({
      "/api/playlists/4/tracks": () => new Response(JSON.stringify({ id: 4, name: "Mix", track_ids: [x.id, y.id] }), { status: 200 }),
    });
    mountApp(TrackRow, { track: x });
    usePlaylistsStore().items = [{ id: 4, name: "Mix", track_ids: [x.id] }];
    const actions = useLibraryActions();
    await actions.addToPlaylist(4, [x, y]);
    const put = calls.filter((c) => c.init?.method === "PUT");
    expect(put).toHaveLength(1);
    expect(JSON.parse(String(put[0].init!.body))).toMatchObject({ track_ids: [y.id] });

    await actions.addToPlaylist(4, [x]);
    expect(calls.filter((c) => c.init?.method === "PUT")).toHaveLength(1);
    expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "info", title: "Already in “Mix”" });
  });
});
