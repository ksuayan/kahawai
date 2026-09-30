import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  checkHealth,
  fetchAlbumDetail,
  fetchAllAlbums,
  fetchArtistDetail,
  fetchArtists,
  fetchGenres,
  fetchTrack,
  searchTracks,
} from "../api";
import { catalogAlbumTracks, catalogCached, catalogSync, catalogTracks, type CachedCatalog } from "../tauri";
import type { Album, Artist, Genre, Track } from "../types";
import { uiGet, uiSet } from "../lib/uiState";

const SEARCH_KEY = "kahawai.search";

function sortTracks(tracks: Track[]): Track[] {
  return [...tracks].sort(
    (a, b) =>
      (a.disc_no ?? 1) - (b.disc_no ?? 1) || (a.track_no ?? 0) - (b.track_no ?? 0),
  );
}

export const useLibraryStore = defineStore("library", () => {
  const albums = ref<Album[]>([]);
  const artists = ref<Artist[]>([]);
  /** Canonical genres, most tracks first (empty on servers without them). */
  const genres = ref<Genre[]>([]);
  const trackCache = ref(new Map<number, Track>());
  /** The last search, remembered across launches (results are fetched again). */
  const searchQuery = ref(uiGet(SEARCH_KEY) ?? "");
  const searchResults = ref<Track[]>([]);
  const searching = ref(false);
  const loading = ref(false);
  const serverOnline = ref<boolean | null>(null);
  const error = ref<string | null>(null);
  /** True while the lists come from the local cache and the server can't
   *  be reached (browse works; playback needs the server). */
  const showingCached = ref(false);

  // Sort keys from the server ("Beatles, The"), falling back to the display
  // strings for an older server.
  const sortedAlbums = computed(() =>
    [...albums.value].sort((a, b) =>
      (a.sort_artist ?? a.artist ?? "").localeCompare(b.sort_artist ?? b.artist ?? "") ||
      (a.year ?? 0) - (b.year ?? 0) ||
      (a.sort_title ?? a.title).localeCompare(b.sort_title ?? b.title),
    ),
  );

  /** Album id → cover hash, for track lists (a lookup per row, not a search). */
  const albumArtwork = computed(() => new Map(albums.value.map((a) => [a.id, a.artwork_hash ?? null])));

  function artworkFor(track: Track): string | null {
    return track.album_id != null ? (albumArtwork.value.get(track.album_id) ?? null) : null;
  }

  const sortedArtists = computed(() =>
    [...artists.value].sort((a, b) => (a.sort_name ?? a.name).localeCompare(b.sort_name ?? b.name)),
  );

  function cacheTracks(tracks: Track[]): void {
    for (const t of tracks) trackCache.value.set(t.id, t);
  }

  function applyCached(c: CachedCatalog): void {
    albums.value = c.albums;
    artists.value = c.artists;
    genres.value = c.genres;
  }

  /**
   * In the app: render from the local catalog cache at once, then sync it
   * with the server (one small request when nothing changed, else a delta
   * or a full pull) and re-render if anything came in. Server unreachable:
   * keep showing the cache. Outside the app, or against a server without
   * the catalog endpoint, fetch the lists directly as before.
   */
  async function loadAll(): Promise<void> {
    loading.value = true;
    error.value = null;
    const cached = await catalogCached();
    const haveCache = cached !== undefined && cached.rev !== null;
    if (haveCache) {
      applyCached(cached);
      loading.value = false;
    }
    const report = await catalogSync();
    if (report && report.status !== "unsupported") {
      if (report.status === "offline") {
        serverOnline.value = false;
        showingCached.value = haveCache;
        if (!haveCache) error.value = "Server not reachable";
      } else {
        serverOnline.value = true;
        showingCached.value = false;
        if (report.status !== "unchanged" || !haveCache) {
          const fresh = await catalogCached();
          if (fresh) applyCached(fresh);
        }
      }
      loading.value = false;
      return;
    }
    await loadFromServer();
  }

  async function loadFromServer(): Promise<void> {
    loading.value = true;
    error.value = null;
    showingCached.value = false;
    try {
      const online = await checkHealth();
      serverOnline.value = online;
      if (!online) throw new Error("Server not reachable");
      // Genres are a nice-to-have: failing to load them doesn't fail the library.
      const [a, ar, g] = await Promise.all([
        fetchAllAlbums(),
        fetchArtists(),
        fetchGenres().catch(() => [] as Genre[]),
      ]);
      albums.value = a;
      artists.value = ar;
      genres.value = g;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  /** Fetch any missing tracks and return them in id order. */
  async function ensureTracks(ids: number[]): Promise<Track[]> {
    let missing = ids.filter((id) => !trackCache.value.has(id));
    if (missing.length > 0 && showingCached.value) {
      // Offline: the local catalog has every present track.
      cacheTracks((await catalogTracks(missing)) ?? []);
      missing = ids.filter((id) => !trackCache.value.has(id));
    }
    if (missing.length > 0) {
      const fetched = await Promise.all(missing.map((id) => fetchTrack(id)));
      cacheTracks(fetched);
    }
    return ids
      .map((id) => trackCache.value.get(id))
      .filter((t): t is Track => t !== undefined);
  }

  /** Full album detail: album record + its tracks, sorted by disc/track. */
  async function getAlbumDetail(id: number): Promise<{ album: Album; tracks: Track[] }> {
    let detail;
    try {
      detail = await fetchAlbumDetail(id);
    } catch (e) {
      // Offline: the album and its tracks from the local catalog.
      const album = albums.value.find((a) => a.id === id);
      const cachedTracks = album ? await catalogAlbumTracks(id) : undefined;
      if (!album || !cachedTracks) throw e;
      cacheTracks(cachedTracks);
      return { album, tracks: sortTracks(cachedTracks) };
    }
    const { album, tracks: served } = detail;
    // The server ships the tracks with the album; cache them and only
    // fetch individually if it left any out.
    cacheTracks(served);
    const tracks = sortTracks(await ensureTracks(album.track_ids ?? served.map((t) => t.id)));
    return { album, tracks };
  }

  async function getArtistAlbums(id: number): Promise<{ artist: Artist; albums: Album[] }> {
    let detail;
    try {
      detail = await fetchArtistDetail(id);
    } catch (e) {
      // Offline: the artist's albums by name, from the cached lists.
      const artist = artists.value.find((a) => a.id === id);
      if (!artist || albums.value.length === 0) throw e;
      return { artist, albums: albums.value.filter((a) => a.artist === artist.name) };
    }
    if (detail.albums) return { artist: detail.artist, albums: detail.albums };
    // Fallback: the endpoint returned just the artist; filter locally.
    if (albums.value.length === 0) albums.value = await fetchAllAlbums();
    return {
      artist: detail.artist,
      albums: albums.value.filter((a) => a.artist === detail.artist.name),
    };
  }

  let searchTimer: number | undefined;
  function search(q: string): void {
    searchQuery.value = q;
    uiSet(SEARCH_KEY, q.trim() ? q : null);
    window.clearTimeout(searchTimer);
    if (!q.trim()) {
      searchResults.value = [];
      searching.value = false;
      return;
    }
    searching.value = true;
    searchTimer = window.setTimeout(async () => {
      try {
        const results = await searchTracks(q.trim());
        cacheTracks(results);
        // Only apply if the query didn't change while fetching.
        if (searchQuery.value === q) searchResults.value = results;
      } catch (e) {
        console.warn("[search]", e);
      } finally {
        if (searchQuery.value === q) searching.value = false;
      }
    }, 250);
  }

  return {
    albums,
    artists,
    genres,
    trackCache,
    searchQuery,
    searchResults,
    searching,
    loading,
    serverOnline,
    showingCached,
    error,
    sortedAlbums,
    sortedArtists,
    artworkFor,
    loadAll,
    cacheTracks,
    ensureTracks,
    getAlbumDetail,
    getArtistAlbums,
    search,
  };
});
