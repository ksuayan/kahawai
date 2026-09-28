import { beforeEach, describe, expect, it } from "vitest";
import {
  ApiError,
  artworkSrc,
  checkHealth,
  fetchAlbumDetail,
  fetchAllAlbums,
  fetchArtistDetail,
  fetchArtists,
  onCatalogUpdated,
  searchTracks,
  setBaseUrl,
} from "./api";
import { makeAlbum, makeTrack, mockFetch } from "./test/fixtures";
import { latestEventSource } from "./test/eventsource-mock";

beforeEach(() => setBaseUrl("http://server:8080"));

describe("base URL", () => {
  it("trims whitespace and trailing slashes, and falls back to localhost", () => {
    setBaseUrl("  http://a:1///  ");
    expect(artworkSrc("abc")).toBe("http://a:1/api/artwork/abc");
    setBaseUrl("   ");
    expect(artworkSrc("abc")).toBe("http://localhost:8080/api/artwork/abc");
  });
});

describe("catalog-updated notifications (SSE)", () => {
  it("opens /api/events against the current base URL", () => {
    expect(latestEventSource()?.url).toBe("http://server:8080/api/events");
  });

  it("reconnects to the new URL when the base URL changes, closing the old connection", () => {
    const first = latestEventSource()!;
    setBaseUrl("http://other:9090");
    expect(first.closed).toBe(true);
    expect(latestEventSource()).not.toBe(first);
    expect(latestEventSource()?.url).toBe("http://other:9090/api/events");
  });

  it("notifies every subscriber when the server emits catalog-updated", () => {
    let calls = 0;
    const stop = onCatalogUpdated(() => calls++);
    latestEventSource()!.emit("catalog-updated");
    expect(calls).toBe(1);
    stop();
    latestEventSource()!.emit("catalog-updated");
    expect(calls).toBe(1); // unsubscribed — no further calls
  });
});

describe("artworkSrc", () => {
  it("returns an empty string without a hash", () => {
    expect(artworkSrc(null)).toBe("");
    expect(artworkSrc(undefined)).toBe("");
    expect(artworkSrc("")).toBe("");
  });

  it("uses the server's /api/artwork route outside Tauri (regression: was /artwork)", () => {
    expect(artworkSrc("abc123")).toBe("http://server:8080/api/artwork/abc123");
  });

  it("goes through the shell's artwork:// protocol (disk cache) inside Tauri", () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    expect(artworkSrc("abc123")).toBe("artwork://localhost/abc123");
  });
});

describe("fetchAlbumDetail", () => {
  it("unwraps the server's {album, tracks} envelope (regression: album.track_ids was undefined)", async () => {
    const t = makeTrack();
    const album = makeAlbum({ id: 9, track_ids: [t.id] });
    mockFetch({ "/api/albums/9": { album, tracks: [t] } });
    const d = await fetchAlbumDetail(9);
    expect(d.album.track_ids).toEqual([t.id]);
    expect(d.tracks).toEqual([t]);
  });

  it("tolerates a bare album (older server)", async () => {
    const album = makeAlbum({ id: 3, track_ids: [1, 2] });
    mockFetch({ "/api/albums/3": album });
    const d = await fetchAlbumDetail(3);
    expect(d.album.id).toBe(3);
    expect(d.tracks).toEqual([]);
  });
});

describe("fetchAllAlbums", () => {
  it("walks pages until a short page arrives", async () => {
    const pages: Record<string, unknown> = {
      "1": { items: [makeAlbum(), makeAlbum()], page: 1, per_page: 2, total: 3 },
      "2": { items: [makeAlbum()], page: 2, per_page: 2, total: 3 },
    };
    const calls = mockFetch({
      "/api/albums": (url: string) => {
        const page = new URL(url).searchParams.get("page")!;
        return new Response(JSON.stringify(pages[page]), { status: 200 });
      },
    });
    const all = await fetchAllAlbums(2);
    expect(all).toHaveLength(3);
    expect(calls.map((c) => new URL(c.url).searchParams.get("page"))).toEqual(["1", "2"]);
  });

  it("accepts a bare array response", async () => {
    mockFetch({ "/api/albums": [makeAlbum(), makeAlbum()] });
    expect(await fetchAllAlbums()).toHaveLength(2);
  });
});

describe("artists and search shapes", () => {
  it("accepts arrays or {items}", async () => {
    mockFetch({ "/api/artists": [{ id: 1, name: "A" }] });
    expect(await fetchArtists()).toEqual([{ id: 1, name: "A" }]);
    mockFetch({ "/api/artists": { items: [{ id: 2, name: "B" }] } });
    expect(await fetchArtists()).toEqual([{ id: 2, name: "B" }]);
  });

  it("URL-encodes the search query", async () => {
    const calls = mockFetch({ "/api/search": [] });
    await searchTracks("Doo-Wops & Hooligans");
    expect(calls[0].url).toContain("q=Doo-Wops%20%26%20Hooligans");
  });

  it("normalises the artist detail shapes", async () => {
    const album = makeAlbum();
    mockFetch({ "/api/artists/1": { artist: { id: 1, name: "A" }, albums: [album] } });
    expect((await fetchArtistDetail(1)).albums).toEqual([album]);
    mockFetch({ "/api/artists/2": { id: 2, name: "B" } });
    const d = await fetchArtistDetail(2);
    expect(d.artist.name).toBe("B");
    expect(d.albums).toBeUndefined();
    mockFetch({ "/api/artists/3": { nonsense: true } });
    await expect(fetchArtistDetail(3)).rejects.toThrow(/unexpected/);
  });
});

describe("errors and health", () => {
  it("surfaces the server's error message verbatim", async () => {
    mockFetch({
      "/api/albums/1": () => new Response(JSON.stringify({ error: "album 1 not found" }), { status: 404 }),
    });
    const err = await fetchAlbumDetail(1).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.status).toBe(404);
    expect(err.message).toBe("album 1 not found");
  });

  it("checkHealth is true only for status ok, false on any failure", async () => {
    mockFetch({ "/api/health": { status: "ok", version: "1" } });
    expect(await checkHealth()).toBe(true);
    mockFetch({ "/api/health": { status: "degraded" } });
    expect(await checkHealth()).toBe(false);
    (globalThis as unknown as { fetch: unknown }).fetch = async () => {
      throw new Error("network down");
    };
    expect(await checkHealth()).toBe(false);
  });
});
