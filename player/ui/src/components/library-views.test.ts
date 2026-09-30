import { describe, expect, it } from "vitest";
import { makeAlbum, makeTrack, mockFetch } from "../test/fixtures";
import { mountApp, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { ResizeObserverStub } from "../test/resizeobserver-mock";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import { useViewPrefsStore } from "../stores/viewPrefs";
import AlbumDetail from "./AlbumDetail.vue";
import AlbumsView from "./AlbumsView.vue";
import ArtistDetail from "./ArtistDetail.vue";
import ArtistsView from "./ArtistsView.vue";
import SearchView from "./SearchView.vue";
import { vi } from "vitest";

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

describe("AlbumsView", () => {
  const seed = (albums: ReturnType<typeof makeAlbum>[]) => () => {
    useLibraryStore().albums = albums;
  };

  // Virtualization (@tanstack/vue-virtual) realizes its first rows via a
  // watcher that fires once the scroll element ref attaches (a microtask,
  // Vue's default 'pre' flush) — one tick later than the old plain v-for,
  // hence the `await settle()` these didn't need before.

  it("shows a card per album with artist, year and a correctly pluralised track count", async () => {
    const { wrapper } = mountApp(AlbumsView, {}, {}, seed([
      makeAlbum({ title: "One Track", artist: "A", year: 2001, track_count: 1 }),
      makeAlbum({ title: "Three Tracks", artist: "B", year: null, track_count: 3 }),
    ]));
    await settle();
    expect(wrapper.text()).toContain("2 albums in library");
    expect(wrapper.text()).toContain("A · 2001 · 1 track");
    expect(wrapper.text()).not.toContain("1 tracks");
    expect(wrapper.text()).toContain("B · 3 tracks");
  });

  it("sorts by artist, then year", async () => {
    const { wrapper } = mountApp(AlbumsView, {}, {}, seed([
      makeAlbum({ title: "Late", artist: "Z", year: 2020 }),
      makeAlbum({ title: "Second", artist: "A", year: 2010 }),
      makeAlbum({ title: "First", artist: "A", year: 2000 }),
    ]));
    await settle();
    expect(wrapper.findAll(".font-semibold.truncate").map((t) => t.text())).toEqual(["First", "Second", "Late"]);
  });

  it("opens an album when its card is clicked", async () => {
    const album = makeAlbum({ id: 77 });
    const { wrapper } = mountApp(AlbumsView, {}, {}, seed([album]));
    await settle();
    await wrapper.findAll("button").find((b) => b.text().includes(album.title))!.trigger("click");
    expect(useNavStore().view).toEqual({ name: "album", id: 77 });
  });

  it("shows albums as a list, in the chosen order", async () => {
    const albums = [
      makeAlbum({ id: 1, title: "Kind of Blue", artist: "Miles Davis", year: 1959 }),
      makeAlbum({ id: 2, title: "Bitches Brew", artist: "Miles Davis", year: 1970 }),
    ];
    const { wrapper } = mountApp(AlbumsView, {}, {}, () => {
      seed(albums)();
      useViewPrefsStore().prefs.albumsLayout = "list";
      useViewPrefsStore().prefs.albumsSort = "year-desc";
    });
    await settle();
    const rows = wrapper.findAll('[data-testid="album-row"]');
    expect(rows.map((r) => r.text().split("Miles")[0])).toEqual(["Bitches Brew", "Kind of Blue"]);
    await rows[1].trigger("click");
    expect(useNavStore().view).toEqual({ name: "album", id: 1 });
  });

  it("shows loading, error and empty states", () => {
    const loading = mountApp(AlbumsView, {}, {}, () => { useLibraryStore().loading = true; }).wrapper;
    expect(loading.text()).toContain("Loading albums…");
    const error = mountApp(AlbumsView, {}, {}, () => { useLibraryStore().error = "Server not reachable"; }).wrapper;
    expect(error.get('[role="alert"]').text()).toBe("Server not reachable");
    expect(mountApp(AlbumsView).wrapper.text()).toContain("No albums found.");
  });

  // Regression, confirmed live (window resized from 900px to 1900px, grid
  // stayed at the same column count with each cell stretched instead of
  // more columns appearing): the grid's resize observer used to attach
  // once, in onMounted. Mounting while the library was still loading meant
  // the scroll element (on the `v-else` branch) wasn't there yet, so that
  // one attempt was a silent no-op — nothing ever retried once the grid
  // appeared, and it stayed pinned at containerWidth's seeded default width
  // forever, never reflecting the real window size.
  it("reflows to more (not larger) columns as the container widens, even when mounted while the library was still loading", async () => {
    const albums = Array.from({ length: 20 }, (_, i) => makeAlbum({ id: i, title: `Album ${i}` }));
    const { wrapper } = mountApp(AlbumsView, {}, {}, () => {
      useLibraryStore().loading = true;
    });
    await settle();
    expect(wrapper.text()).toContain("Loading albums…"); // the buggy scenario: not there yet

    useLibraryStore().loading = false;
    useLibraryStore().albums = albums;
    await settle();

    const scrollEl = wrapper.find(".overflow-y-auto").element;
    // @tanstack/vue-virtual also resize-observes this same element (for its
    // own viewport tracking), so more than one stub instance may exist —
    // trigger every one watching it, the same as a real window resize would
    // fire every observer on that element, not just "the latest" mock.
    const lanesAt = async (width: number): Promise<number> => {
      for (const ro of ResizeObserverStub.instances) ro.trigger(scrollEl, width);
      await settle();
      const grid = wrapper.find(".grid");
      const cols = (grid.attributes("style") ?? "").match(/repeat\((\d+),/);
      return cols ? Number(cols[1]) : 0;
    };
    const narrow = await lanesAt(900);
    const wide = await lanesAt(1900);
    expect(wide).toBeGreaterThan(narrow);
  });

  // The grid's lane count is computed from the container's actual width, so
  // giving it the full panel (not a fixed max-width like the other views)
  // is what lets more columns show up on a wide display instead of the
  // grid capping out regardless of window size.
  it("takes the full panel width instead of capping out like other views", () => {
    const { wrapper } = mountApp(AlbumsView, {}, {}, seed([makeAlbum()]));
    expect(wrapper.get(".px-6.pb-10.pt-5").classes()).toContain("max-w-none");
  });
});

describe("ArtistsView", () => {
  it("lists artists alphabetically with an initial and opens one on click", async () => {
    const { wrapper } = mountApp(ArtistsView, {}, {}, () => {
      useLibraryStore().artists = [{ id: 2, name: "Stan Getz" }, { id: 1, name: "Bruno Mars" }];
    });
    expect(wrapper.text()).toContain("2 artists");
    await settle();
    const rows = wrapper.findAll("button").filter((b) => /Bruno|Stan/.test(b.text()));
    expect(rows.map((r) => r.text())).toEqual(["BBruno Mars", "SStan Getz"]);
    await rows[1].trigger("click");
    expect(useNavStore().view).toEqual({ name: "artist", id: 2 });
  });

  it("shows loading, error and empty states", () => {
    expect(mountApp(ArtistsView, {}, {}, () => { useLibraryStore().loading = true; }).wrapper.text()).toContain("Loading artists…");
    expect(mountApp(ArtistsView, {}, {}, () => { useLibraryStore().error = "boom"; }).wrapper.get('[role="alert"]').text()).toBe("boom");
    expect(mountApp(ArtistsView).wrapper.text()).toContain("No artists found.");
  });
});

describe("ArtistDetail", () => {
  it("shows the artist's albums, oldest first, and opens one", async () => {
    mockFetch({
      "/api/artists/1": {
        artist: { id: 1, name: "Phoebe Bridgers" },
        albums: [makeAlbum({ id: 20, title: "Punisher", year: 2020, track_count: 11 }), makeAlbum({ id: 10, title: "Stranger in the Alps", year: 2017, track_count: 11 })],
      },
    });
    const { wrapper } = mountApp(ArtistDetail, { id: 1 });
    await settle();
    expect(wrapper.get("h2").text()).toBe("Phoebe Bridgers");
    expect(wrapper.text()).toContain("2 albums");
    expect(wrapper.findAll(".font-semibold.truncate").map((t) => t.text())).toEqual(["Stranger in the Alps", "Punisher"]);
    await wrapper.findAll(".group")[0].trigger("click");
    expect(useNavStore().view).toEqual({ name: "album", id: 10 });
  });

  it("handles an artist with no albums, a single album, and errors", async () => {
    mockFetch({ "/api/artists/1": { artist: { id: 1, name: "Solo" }, albums: [] } });
    let w = mountApp(ArtistDetail, { id: 1 }).wrapper;
    await settle();
    expect(w.text()).toContain("No albums found for this artist.");

    mockFetch({ "/api/artists/2": { artist: { id: 2, name: "One" }, albums: [makeAlbum()] } });
    w = mountApp(ArtistDetail, { id: 2 }).wrapper;
    await settle();
    expect(w.text()).toContain("1 album");
    expect(w.text()).not.toContain("1 albums");

    mockFetch({ "/api/artists/3": () => json({ error: "artist 3 not found" }, 404) });
    w = mountApp(ArtistDetail, { id: 3 }).wrapper;
    await settle();
    expect(w.get('[role="alert"]').text()).toBe("artist 3 not found");
  });

  it("goes back to the artist list", async () => {
    mockFetch({ "/api/artists/1": { artist: { id: 1, name: "A" }, albums: [] } });
    const { wrapper } = mountApp(ArtistDetail, { id: 1 });
    await settle();
    await wrapper.findAll("button").find((b) => b.text().includes("Artists"))!.trigger("click");
    expect(useNavStore().view.name).toBe("artists");
  });
});

describe("AlbumDetail", () => {
  const t1 = makeTrack({ title: "Smoke Signals", track_no: 1, duration_ms: 318_000 });
  const t2 = makeTrack({ title: "Demi Moore", track_no: 4, duration_ms: 196_000 });
  const t3 = makeTrack({ title: "Reprise", track_no: 11, duration_ms: 44_000, missing: true });
  const album = makeAlbum({ id: 5, title: "Stranger in the Alps", artist: "Phoebe Bridgers", year: 2017, track_ids: [t1.id, t2.id, t3.id] });
  const routes = () => mockFetch({ "/api/albums/5": { album, tracks: [t1, t2, t3] }, "/api/playlists": [] });

  it("shows the album header (artist · year, track count, total time) and every track in order", async () => {
    routes();
    const { wrapper } = mountApp(AlbumDetail, { id: 5 });
    await settle();
    expect(wrapper.get("h2").text()).toBe("Stranger in the Alps");
    expect(wrapper.text()).toContain("Phoebe Bridgers · 2017");
    expect(wrapper.text()).toContain("3 tracks · 9:18");
    expect(wrapper.findAll("[data-playable]").map((r) => r.find(".truncate").text())).toEqual(["Smoke Signals", "Demi Moore", "Reprise"]);
  });

  it("offers Add to queue / Add to playlist… buttons instead of a ⋯ menu, adding only playable tracks", async () => {
    routes();
    const { wrapper } = mountApp(AlbumDetail, { id: 5 });
    await settle();
    expect(wrapper.get('[data-testid="add-to-playlist"]').text()).toContain("Add to playlist");
    expect(wrapper.find('[aria-label="Actions"]').exists()).toBe(false);
    await wrapper.get('[data-testid="add-to-queue"]').trigger("click");
    await settle();
    const sent = (tauri.callsTo("queue_append")[0] as { tracks: { id: number }[] }).tracks.map((t) => t.id);
    expect(sent).toEqual([t1.id, t2.id]);
  });

  it("Play queues the playable tracks from the start (skipping missing files)", async () => {
    routes();
    const { wrapper } = mountApp(AlbumDetail, { id: 5 });
    await settle();
    await wrapper.findAll("button").find((b) => b.text().includes("Play"))!.trigger("click");
    await settle();
    const call = tauri.callsTo("queue_play")[0] as { tracks: { id: number }[]; index: number };
    expect(call.tracks.map((t) => t.id)).toEqual([t1.id, t2.id]);
    expect(call.index).toBe(0);
    expect(useQueueStore().tracks).toHaveLength(2);
  });

  it("disables Play when nothing is playable", async () => {
    mockFetch({ "/api/albums/6": { album: makeAlbum({ id: 6, track_ids: [t3.id] }), tracks: [t3] }, "/api/playlists": [] });
    const { wrapper } = mountApp(AlbumDetail, { id: 6 });
    await settle();
    expect(wrapper.findAll("button").find((b) => b.text().includes("Play"))!.attributes("disabled")).toBeDefined();
  });

  it("double-click plays from that track", async () => {
    routes();
    const { wrapper } = mountApp(AlbumDetail, { id: 5 });
    await settle();
    await wrapper.findAll("[data-playable]")[1].trigger("dblclick");
    await settle();
    expect((tauri.callsTo("queue_play")[0] as { index: number }).index).toBe(1);
  });

  it("highlights the track that is playing", async () => {
    routes();
    const { wrapper } = mountApp(AlbumDetail, { id: 5 }, {}, () => {
      useQueueStore().tracks = [t2];
      useQueueStore().index = 0;
    });
    await settle();
    expect(wrapper.findAll("[data-playable]").map((r) => r.attributes("data-current") !== undefined)).toEqual([false, true, false]);
  });

  it("reloads when the album id changes", async () => {
    routes();
    const { wrapper } = mountApp(AlbumDetail, { id: 5 });
    await settle();
    mockFetch({ "/api/albums/9": { album: makeAlbum({ id: 9, title: "Other", track_ids: [] }), tracks: [] }, "/api/playlists": [] });
    await wrapper.setProps({ id: 9 });
    await settle();
    expect(wrapper.get("h2").text()).toBe("Other");
  });

  it("shows the server's error and goes back to albums", async () => {
    mockFetch({ "/api/albums/5": () => json({ error: "album 5 not found" }, 404) });
    const { wrapper } = mountApp(AlbumDetail, { id: 5 });
    await settle();
    expect(wrapper.get('[role="alert"]').text()).toBe("album 5 not found");
    await wrapper.findAll("button").find((b) => b.text().includes("Albums"))!.trigger("click");
    expect(useNavStore().view.name).toBe("albums");
  });
});

describe("SearchView", () => {
  it("focuses the search box when it opens", async () => {
    const { wrapper } = mountApp(SearchView);
    await settle();
    expect(document.activeElement).toBe(wrapper.get("input").element);
  });

  it("searches as you type (debounced) and lists the results", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const hit = makeTrack({ title: "Grenade", artist: "Bruno Mars" });
    const calls = mockFetch({ "/api/search": [hit], "/api/playlists": [] });
    const { wrapper } = mountApp(SearchView);
    await typeInto(wrapper.get("input").element as HTMLInputElement, "gren");
    expect(wrapper.text()).toContain("Searching…");
    await vi.advanceTimersByTimeAsync(300);
    await settle();
    expect(calls.filter((c) => c.url.includes("/api/search"))).toHaveLength(1);
    expect(wrapper.get("[data-playable]").text()).toContain("Grenade");
    expect(wrapper.text()).not.toContain("Searching…");
  });

  it("says when nothing matches, quoting the query", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    mockFetch({ "/api/search": [] });
    const { wrapper } = mountApp(SearchView);
    await typeInto(wrapper.get("input").element as HTMLInputElement, "zzz");
    await vi.advanceTimersByTimeAsync(300);
    await settle();
    expect(wrapper.text()).toContain('No tracks match "zzz".');
  });

  it("plays from a result, queueing every playable result", async () => {
    const a = makeTrack({ title: "A" });
    const b = makeTrack({ title: "B" });
    const c = makeTrack({ title: "C", missing: true });
    const { wrapper } = mountApp(SearchView, {}, {}, () => {
      mockFetch({ "/api/playlists": [] });
      useLibraryStore().searchQuery = "x";
      useLibraryStore().searchResults = [a, b, c];
    });
    await settle();
    await wrapper.findAll("[data-playable]")[1].trigger("dblclick");
    await settle();
    const call = tauri.callsTo("queue_play")[0] as { tracks: { id: number }[]; index: number };
    expect(call.tracks.map((t) => t.id)).toEqual([a.id, b.id]);
    expect(call.index).toBe(1);
  });

  it("clears results for a blank query", async () => {
    const { wrapper } = mountApp(SearchView, {}, {}, () => {
      mockFetch({ "/api/playlists": [] });
      useLibraryStore().searchResults = [makeTrack()];
    });
    await typeInto(wrapper.get("input").element as HTMLInputElement, "   ");
    await settle();
    expect(useLibraryStore().searchResults).toEqual([]);
  });
});

describe("track views: list or grid, and sort", () => {
  it("an album's tracks follow the chosen order, and Play plays them in it", async () => {
    const album = makeAlbum({ id: 5, title: "Mix" });
    const t1 = makeTrack({ id: 11, album_id: 5, track_no: 1, title: "One", artist: "Zed" });
    const t2 = makeTrack({ id: 12, album_id: 5, track_no: 2, title: "Two", artist: "Abe" });
    mockFetch({ "/api/albums/5": { album: { ...album, track_ids: [11, 12] }, tracks: [t1, t2] }, "/api/playlists": [] });
    const { wrapper } = mountApp(AlbumDetail, { id: 5 }, {}, () => {
      useViewPrefsStore().prefs.albumTracksSort = "artist-asc";
    });
    await settle();
    const titles = wrapper.findAll("[data-playable]").map((r) => (r.text().includes("Two") ? "Two" : "One"));
    expect(titles).toEqual(["Two", "One"]);
    await wrapper.findAll("button").find((b) => b.text() === "Play")!.trigger("click");
    await settle();
    expect(useQueueStore().tracks.map((t) => t.id)).toEqual([12, 11]);
  });

  it("an album's tracks as a grid of cards", async () => {
    const album = makeAlbum({ id: 6, title: "Grid" });
    const t = makeTrack({ id: 21, album_id: 6, title: "Only" });
    mockFetch({ "/api/albums/6": { album: { ...album, track_ids: [21] }, tracks: [t] }, "/api/playlists": [] });
    const { wrapper } = mountApp(AlbumDetail, { id: 6 }, {}, () => {
      useViewPrefsStore().prefs.albumTracksLayout = "grid";
    });
    await settle();
    expect(wrapper.findAll('[data-testid="track-card"]').map((c) => c.text())).toEqual([expect.stringContaining("Only")]);
  });

  it("search results sort and switch to a grid", async () => {
    const { wrapper } = mountApp(SearchView, {}, {}, () => {
      const lib = useLibraryStore();
      lib.searchQuery = "x";
      lib.searchResults = [makeTrack({ title: "Late", year: 2020 }), makeTrack({ title: "Early", year: 1960 })];
      useViewPrefsStore().prefs.searchSort = "year-asc";
      useViewPrefsStore().prefs.searchLayout = "grid";
    });
    await settle();
    const cards = wrapper.findAll('[data-testid="track-card"]');
    expect(cards.map((c) => (c.text().includes("Early") ? "Early" : "Late"))).toEqual(["Early", "Late"]);
  });
});
