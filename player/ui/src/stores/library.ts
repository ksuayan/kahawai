import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  checkHealth,
  fetchAlbum,
  fetchAllAlbums,
  fetchArtistDetail,
  fetchArtists,
  fetchTrack,
  searchTracks,
} from "../api";
import type { Album, Artist, Track } from "../types";

function sortTracks(tracks: Track[]): Track[] {
  return [...tracks].sort(
    (a, b) =>
      (a.disc_no ?? 1) - (b.disc_no ?? 1) || (a.track_no ?? 0) - (b.track_no ?? 0),
  );
}

export const useLibraryStore = defineStore("library", () => {
  const albums = ref<Album[]>([]);
  const artists = ref<Artist[]>([]);
  const trackCache = ref(new Map<number, Track>());
  const searchQuery = ref("");
  const searchResults = ref<Track[]>([]);
  const searching = ref(false);
  const loading = ref(false);
  const serverOnline = ref<boolean | null>(null);
  const error = ref<string | null>(null);

  const sortedAlbums = computed(() =>
    [...albums.value].sort((a, b) =>
      (a.artist ?? "").localeCompare(b.artist ?? "") ||
      (a.year ?? 0) - (b.year ?? 0) ||
      a.title.localeCompare(b.title),
    ),
  );

  const sortedArtists = computed(() =>
    [...artists.value].sort((a, b) => a.name.localeCompare(b.name)),
  );

  function cacheTracks(tracks: Track[]): void {
    for (const t of tracks) trackCache.value.set(t.id, t);
  }

  async function loadAll(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const online = await checkHealth();
      serverOnline.value = online;
      if (!online) throw new Error("Server not reachable");
      const [a, ar] = await Promise.all([fetchAllAlbums(), fetchArtists()]);
      albums.value = a;
      artists.value = ar;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  /** Fetch any missing tracks and return them in id order. */
  async function ensureTracks(ids: number[]): Promise<Track[]> {
    const missing = ids.filter((id) => !trackCache.value.has(id));
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
    const album = await fetchAlbum(id);
    const tracks = sortTracks(await ensureTracks(album.track_ids));
    return { album, tracks };
  }

  async function getArtistAlbums(id: number): Promise<{ artist: Artist; albums: Album[] }> {
    const detail = await fetchArtistDetail(id);
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
    trackCache,
    searchQuery,
    searchResults,
    searching,
    loading,
    serverOnline,
    error,
    sortedAlbums,
    sortedArtists,
    loadAll,
    cacheTracks,
    ensureTracks,
    getAlbumDetail,
    getArtistAlbums,
    search,
  };
});
