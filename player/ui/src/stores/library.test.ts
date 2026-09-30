import { flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBaseUrl } from "../api";
import { makeAlbum, makeTrack, mockFetch } from "../test/fixtures";
import { useLibraryStore } from "./library";

beforeEach(() => {
  setActivePinia(createPinia());
  setBaseUrl("http://server:8080");
});
afterEach(() => vi.useRealTimers());

describe("loadAll", () => {
  it("loads albums and artists when the server is up", async () => {
    mockFetch({
      "/api/health": { status: "ok" },
      "/api/albums": { items: [makeAlbum({ title: "B" }), makeAlbum({ title: "A" })], page: 1, per_page: 500, total: 2 },
      "/api/artists": [{ id: 1, name: "Zed" }, { id: 2, name: "Abe" }],
    });
    const lib = useLibraryStore();
    await lib.loadAll();
    expect(lib.serverOnline).toBe(true);
    expect(lib.albums).toHaveLength(2);
    expect(lib.sortedArtists.map((a) => a.name)).toEqual(["Abe", "Zed"]);
    expect(lib.loading).toBe(false);
    expect(lib.error).toBeNull();
  });

  it("reports an unreachable server instead of throwing", async () => {
    mockFetch({});
    const lib = useLibraryStore();
    await lib.loadAll();
    expect(lib.serverOnline).toBe(false);
    expect(lib.error).toMatch(/not reachable/i);
    expect(lib.loading).toBe(false);
  });

  it("sorts albums by artist, then year, then title", async () => {
    const lib = useLibraryStore();
    lib.albums = [
      makeAlbum({ artist: "B", year: 2001, title: "x" }),
      makeAlbum({ artist: "A", year: 2010, title: "y" }),
      makeAlbum({ artist: "A", year: 2005, title: "z" }),
      makeAlbum({ artist: "A", year: 2005, title: "a" }),
    ];
    expect(lib.sortedAlbums.map((a) => `${a.artist}${a.year}${a.title}`)).toEqual([
      "A2005a",
      "A2005z",
      "A2010y",
      "B2001x",
    ]);
  });

  it("sorts by the server's sort keys, so a leading 'The' doesn't file under T", async () => {
    const lib = useLibraryStore();
    lib.albums = [
      makeAlbum({ artist: "The Beatles", sort_artist: "Beatles, The", title: "The White Album", sort_title: "White Album, The" }),
      makeAlbum({ artist: "The Beatles", sort_artist: "Beatles, The", title: "Abbey Road", sort_title: "Abbey Road" }),
      makeAlbum({ artist: "Coltrane", title: "Blue Train" }), // older server: no sort keys
    ];
    expect(lib.sortedAlbums.map((a) => a.title)).toEqual(["Abbey Road", "The White Album", "Blue Train"]);
    lib.artists = [
      { id: 1, name: "The Beatles", sort_name: "Beatles, The" },
      { id: 2, name: "Coltrane" },
      { id: 3, name: "Abe" },
    ];
    expect(lib.sortedArtists.map((a) => a.name)).toEqual(["Abe", "The Beatles", "Coltrane"]);
  });
});

describe("getAlbumDetail", () => {
  it("caches the tracks the server ships with the album (one request, no N+1)", async () => {
    const t1 = makeTrack({ track_no: 2, disc_no: 1 });
    const t2 = makeTrack({ track_no: 1, disc_no: 1 });
    const album = makeAlbum({ id: 5, track_ids: [t1.id, t2.id] });
    const calls = mockFetch({ "/api/albums/5": { album, tracks: [t1, t2] } });
    const lib = useLibraryStore();
    const d = await lib.getAlbumDetail(5);
    expect(d.tracks.map((t) => t.id)).toEqual([t2.id, t1.id]); // sorted by disc/track
    expect(calls).toHaveLength(1);
    expect(lib.trackCache.get(t1.id)).toBeDefined();
  });

  it("orders tracks by disc then track number", async () => {
    const a = makeTrack({ disc_no: 2, track_no: 1 });
    const b = makeTrack({ disc_no: 1, track_no: 9 });
    const c = makeTrack({ disc_no: 1, track_no: 3 });
    const album = makeAlbum({ id: 6, track_ids: [a.id, b.id, c.id] });
    mockFetch({ "/api/albums/6": { album, tracks: [a, b, c] } });
    const d = await useLibraryStore().getAlbumDetail(6);
    expect(d.tracks.map((t) => t.id)).toEqual([c.id, b.id, a.id]);
  });

  it("fetches individually any track the server left out", async () => {
    const shipped = makeTrack();
    const left = makeTrack();
    const album = makeAlbum({ id: 7, track_ids: [shipped.id, left.id] });
    const calls = mockFetch({
      "/api/albums/7": { album, tracks: [shipped] },
      [`/api/tracks/${left.id}`]: left,
    });
    const d = await useLibraryStore().getAlbumDetail(7);
    expect(d.tracks.map((t) => t.id)).toEqual([shipped.id, left.id].sort((x, y) => (x === shipped.id ? -1 : 1)));
    expect(calls.map((c) => c.url)).toContain(`http://server:8080/api/tracks/${left.id}`);
  });
});

describe("ensureTracks", () => {
  it("returns cached tracks in the requested order without refetching", async () => {
    const calls = mockFetch({});
    const lib = useLibraryStore();
    const a = makeTrack();
    const b = makeTrack();
    lib.cacheTracks([a, b]);
    const out = await lib.ensureTracks([b.id, a.id]);
    expect(out.map((t) => t.id)).toEqual([b.id, a.id]);
    expect(calls).toHaveLength(0);
  });
});

describe("getArtistAlbums", () => {
  it("uses albums from the artist response when present", async () => {
    const album = makeAlbum();
    mockFetch({ "/api/artists/1": { artist: { id: 1, name: "A" }, albums: [album] } });
    const r = await useLibraryStore().getArtistAlbums(1);
    expect(r.albums).toEqual([album]);
  });

  it("falls back to filtering the album list by artist name", async () => {
    const mine = makeAlbum({ artist: "A" });
    const other = makeAlbum({ artist: "B" });
    mockFetch({
      "/api/artists/2": { id: 2, name: "A" },
      "/api/albums": { items: [mine, other], page: 1, per_page: 500, total: 2 },
    });
    const r = await useLibraryStore().getArtistAlbums(2);
    expect(r.albums.map((a) => a.id)).toEqual([mine.id]);
  });
});

describe("search", () => {
  it("debounces, caches results, and clears on an empty query", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const t = makeTrack({ title: "Grenade" });
    const calls = mockFetch({ "/api/search": [t] });
    const lib = useLibraryStore();
    lib.search("g");
    lib.search("gr");
    lib.search("gren");
    expect(lib.searching).toBe(true);
    await vi.advanceTimersByTimeAsync(300);
    await flushPromises();
    expect(calls).toHaveLength(1); // only the last keystroke searched
    expect(lib.searchResults.map((x) => x.id)).toEqual([t.id]);
    expect(lib.searching).toBe(false);
    expect(lib.trackCache.get(t.id)).toBeDefined();

    lib.search("   ");
    expect(lib.searchResults).toEqual([]);
    expect(lib.searching).toBe(false);
  });

  it("discards a stale response when the query changed while fetching", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const slow = makeTrack({ title: "slow" });
    (globalThis as unknown as { fetch: unknown }).fetch = async () => {
      await gate;
      return new Response(JSON.stringify([slow]), { status: 200 });
    };
    const lib = useLibraryStore();
    lib.search("slow");
    await vi.advanceTimersByTimeAsync(300); // request now in flight
    lib.search("other"); // user kept typing
    release();
    await flushPromises();
    expect(lib.searchResults).toEqual([]);
  });
});
