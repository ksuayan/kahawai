import { flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { makeState, makeTrack, mockFetch } from "../test/fixtures";
import { tauri } from "../test/tauri-mock";
import type { RadioFavorite, RadioStation } from "../types";
import { useAudiobooksStore } from "./audiobooks";
import { usePlayerStore } from "./player";
import { formatOfCodec, isStationTrack, useRadioStore } from "./radio";

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

const fav = (id: number, over: Partial<RadioFavorite> = {}): RadioFavorite => ({
  id,
  station_uuid: `uuid-${id}`,
  name: `Station ${id}`,
  url: `http://s${id}/pls`,
  url_resolved: `http://s${id}/live`,
  homepage: null,
  favicon: null,
  tags: "jazz,smooth",
  country: "France",
  language: null,
  bitrate: 128,
  codec: "AAC+",
  manual: false,
  sort_order: id,
  added_at: 0,
  ...over,
});

const station = (n: number, over: Partial<RadioStation> = {}): RadioStation => ({
  station_uuid: `d-${n}`,
  name: `Directory ${n}`,
  url: `http://d${n}/pls`,
  url_resolved: `http://d${n}/live`,
  homepage: null,
  favicon: null,
  tags: "rock",
  country: "UK",
  language: null,
  codec: "MP3",
  bitrate: 96,
  clicks: 10,
  hls: false,
  needs_relay: false,
  ...over,
});

/** What the engine reports while a station plays. */
function playing(id: number, name: string, radio: Record<string, unknown> | null) {
  const track = makeTrack({ id, title: name, artist: "Live radio", duration_ms: null, album_id: null });
  tauri.emit("player-state", makeState({ status: "playing", track, radio } as never));
}

beforeEach(() => {
  setActivePinia(createPinia());
  tauri.reset();
  tauri.on("get_state", makeState({ status: "stopped", track: null }));
});

describe("helpers", () => {
  it("a negative id is a station, and the codec picks the format", () => {
    expect(isStationTrack(makeTrack({ id: -4 }))).toBe(true);
    expect(isStationTrack(makeTrack({ id: 4 }))).toBe(false);
    expect(isStationTrack(null)).toBe(false);
    expect(formatOfCodec("AAC+")).toBe("aac");
    expect(formatOfCodec("OGG")).toBe("ogg_vorbis");
    expect(formatOfCodec("opus")).toBe("opus");
    expect(formatOfCodec(null)).toBe("mp3");
  });
});

describe("playing a station", () => {
  it("asks the server for the address, then plays it as a one-item queue with a negative id", async () => {
    const calls = mockFetch({
      "/api/radio/favorites/3/play": () => json({ name: "Station 3", url: "http://fresh/live", codec: "AAC+", bitrate: 128, needs_relay: true }),
    });
    await usePlayerStore().init();
    const radio = useRadioStore();
    await radio.play(fav(3));
    expect(calls.some((c) => c.url.endsWith("/api/radio/favorites/3/play") && c.init?.method === "POST")).toBe(true);
    const play = tauri.callsTo("queue_play").at(-1) as { tracks: { id: number; path: string; format: string; title: string; duration_ms: number | null }[]; index: number };
    expect(play.index).toBe(0);
    expect(play.tracks).toHaveLength(1);
    expect(play.tracks[0]).toMatchObject({ id: -3, path: "http://fresh/live", format: "aac", title: "Station 3", duration_ms: null });
  });

  it("a directory station that is not saved plays from its own address", async () => {
    mockFetch({});
    await usePlayerStore().init();
    const radio = useRadioStore();
    await radio.playStation(station(1));
    const t = (tauri.callsTo("queue_play").at(-1) as { tracks: { id: number; path: string; format: string }[] }).tracks[0];
    expect(t.id).toBeLessThan(0);
    expect(t.path).toBe("http://d1/live");
    expect(t.format).toBe("mp3");
  });

  it("a book in progress is put down first, with its place saved and its speed undone", async () => {
    const calls = mockFetch({
      "/api/audiobooks/7/position": () => json({ book_offset_ms: 12_000, updated_at: 1, finished: false }),
    });
    await usePlayerStore().init();
    const books = useAudiobooksStore();
    books.active = { id: 7, duration_ms: 600_000, parts: [], chapters: [], settings: { speed: 1.5 }, position_ms: 0 } as never;
    const radio = useRadioStore();
    await radio.playStation(station(2));
    expect(books.active).toBeNull();
    expect(calls.some((c) => c.url.endsWith("/api/audiobooks/7/position") && c.init?.method === "PUT")).toBe(true);
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1 });
    expect(tauri.callsTo("queue_play")).toHaveLength(1);
  });
});

describe("what the station reports", () => {
  it("shows the live title, keeps the songs heard once each, and tells the server", async () => {
    const calls = mockFetch({ "/api/radio/history": () => json({}, 201) });
    await usePlayerStore().init();
    const radio = useRadioStore();
    playing(-3, "Station 3", { title: null, reconnecting: false, attempt: 0, bitrate_kbps: 128, reason: null });
    await flushPromises();
    expect(radio.isPlaying).toBe(true);
    expect(radio.stationName).toBe("Station 3");
    expect(radio.now?.title).toBeNull();
    playing(-3, "Station 3", { title: "A - One", reconnecting: false, attempt: 0, bitrate_kbps: 128, reason: null });
    await flushPromises();
    playing(-3, "Station 3", { title: "A - One", reconnecting: false, attempt: 0, bitrate_kbps: 128, reason: null });
    playing(-3, "Station 3", { title: "B - Two", reconnecting: false, attempt: 0, bitrate_kbps: 128, reason: null });
    await flushPromises();
    expect(radio.heard).toEqual(["B - Two", "A - One"]);
    const logged = calls.filter((c) => c.url.endsWith("/api/radio/history")).map((c) => JSON.parse(String(c.init?.body)));
    expect(logged).toEqual([
      { station_name: "Station 3", stream_title: "A - One" },
      { station_name: "Station 3", stream_title: "B - Two" },
    ]);
  });

  it("music is not radio", async () => {
    mockFetch({});
    await usePlayerStore().init();
    const radio = useRadioStore();
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ id: 12 }) }));
    await flushPromises();
    expect(radio.isPlaying).toBe(false);
    expect(radio.now).toBeNull();
  });
});

describe("favorites and the directory", () => {
  it("saves a directory station once and can reorder and remove", async () => {
    let list = [fav(1), fav(2)];
    const calls = mockFetch({
      "/api/radio/favorites/order": (_u: string, init?: RequestInit) => {
        const ids = JSON.parse(String(init?.body)).ids as number[];
        list = ids.map((i) => list.find((f) => f.id === i)!);
        return json(list);
      },
      "/api/radio/favorites/2": () => new Response(null, { status: 204 }),
      "/api/radio/favorites": (_u: string, init?: RequestInit) => {
        if (init?.method === "POST") {
          list = [...list, fav(9, { name: "Directory 1", station_uuid: "d-1" })];
          return json(list[list.length - 1], 201);
        }
        return json(list);
      },
    });
    const radio = useRadioStore();
    await radio.loadFavorites();
    expect(radio.favorites.map((f) => f.id)).toEqual([1, 2]);
    await radio.addFavorite(station(1));
    expect(radio.favoriteOf(station(1))?.id).toBe(9);
    await radio.move(9, -1);
    expect(radio.favorites.map((f) => f.id)).toEqual([1, 9, 2]);
    expect(JSON.parse(String(calls.find((c) => c.url.endsWith("/order"))!.init!.body)).ids).toEqual([1, 9, 2]);
    await radio.move(1, -1); // already first: nothing sent
    expect(calls.filter((c) => c.url.endsWith("/order"))).toHaveLength(1);
    await radio.removeFavorite(2);
    expect(calls.some((c) => c.url.endsWith("/favorites/2") && c.init?.method === "DELETE")).toBe(true);
  });

  it("searches with what was typed and notices when the server's directory is off", async () => {
    let off = false;
    const calls = mockFetch({
      "/api/radio/search": () =>
        off ? json({ error: "the online station directory is off: turn on Online sources in the Server settings" }, 400) : json([station(1)]),
    });
    const radio = useRadioStore();
    radio.query.q = "jazz fm";
    radio.query.country = "France";
    await radio.search();
    expect(radio.results).toHaveLength(1);
    const url = calls.find((c) => c.url.includes("/api/radio/search"))!.url;
    expect(url).toContain("q=jazz+fm");
    expect(url).toContain("country=France");
    expect(url).not.toContain("tag=");
    off = true;
    await radio.search();
    expect(radio.directoryOff).toBe(true);
    expect(radio.error).toBeNull();
    expect(radio.results).toEqual([]);
  });

  it("adding by address shows the server's reason when it is not a stream", async () => {
    mockFetch({
      "/api/radio/favorites": (_u: string, init?: RequestInit) =>
        init?.method === "POST" ? json({ error: "that address is a playlist (.pls / .m3u); paste the stream address inside it" }, 400) : json([]),
    });
    const radio = useRadioStore();
    await expect(radio.addByAddress("http://x/list.pls")).rejects.toThrow(/playlist/);
  });
});
