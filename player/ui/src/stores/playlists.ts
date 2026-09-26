import { defineStore } from "pinia";
import { ref } from "vue";
import {
  createPlaylist,
  deletePlaylist,
  fetchPlaylist,
  fetchPlaylists,
  importPlaylistFile,
  renamePlaylist,
  setPlaylistTracks,
} from "../api";
import { useLibraryStore } from "./library";
import { useQueueStore } from "./queue";
import { useToastsStore } from "./toasts";
import type { ImportPlaylistResult, Playlist, Track } from "../types";

export const usePlaylistsStore = defineStore("playlists", () => {
  const items = ref<Playlist[]>([]);
  const detail = ref<{ playlist: Playlist; tracks: Track[] } | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);
  /** Last M3U import result, shown in the import dialog. */
  const lastImport = ref<{ name: string; result: ImportPlaylistResult } | null>(null);

  const toasts = useToastsStore();
  /** Set once a load has completed (successfully or not); guards against
   *  N components each firing a fetch on mount. */
  const loaded = ref(false);
  let inflight: Promise<void> | null = null;

  async function load(): Promise<void> {
    if (loaded.value) return;
    if (inflight) return inflight;
    loading.value = true;
    error.value = null;
    inflight = fetchPlaylists()
      .then((list) => {
        items.value = list;
      })
      .catch((e: unknown) => {
        error.value = e instanceof Error ? e.message : String(e);
      })
      .finally(() => {
        loading.value = false;
        loaded.value = true;
        inflight = null;
      });
    return inflight;
  }

  /** Force a refetch even if a previous load completed. */
  async function reload(): Promise<void> {
    loaded.value = false;
    return load();
  }

  async function open(id: number): Promise<void> {
    loading.value = true;
    error.value = null;
    detail.value = null;
    try {
      const playlist = await fetchPlaylist(id);
      const lib = useLibraryStore();
      const tracks = await lib.ensureTracks(playlist.track_ids);
      detail.value = { playlist, tracks };
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  async function create(name: string, trackIds: number[] = []): Promise<Playlist> {
    const playlist = await createPlaylist({ name, track_ids: trackIds });
    items.value.push(playlist);
    return playlist;
  }

  /** Optimistic rename with rollback on failure. */
  async function rename(id: number, name: string): Promise<void> {
    const item = items.value.find((p) => p.id === id);
    const prev = item?.name;
    if (item) item.name = name;
    if (detail.value?.playlist.id === id) detail.value.playlist.name = name;
    try {
      const updated = await renamePlaylist(id, name);
      if (item) item.name = updated.name;
      if (detail.value?.playlist.id === id) detail.value.playlist.name = updated.name;
    } catch (e) {
      if (item && prev !== undefined) item.name = prev;
      if (detail.value?.playlist.id === id && prev !== undefined) {
        detail.value.playlist.name = prev;
      }
      throw e;
    }
  }

  async function remove(id: number): Promise<void> {
    await deletePlaylist(id);
    items.value = items.value.filter((p) => p.id !== id);
    if (detail.value?.playlist.id === id) detail.value = null;
  }

  /** Replace the open playlist's track order (reorder) or contents. */
  async function replaceTracks(trackIds: number[]): Promise<void> {
    const d = detail.value;
    if (!d) return;
    const prevIds = d.playlist.track_ids;
    const prevTracks = d.tracks;
    const lib = useLibraryStore();
    // Optimistic: reorder locally first.
    const optimistic = await lib.ensureTracks(trackIds);
    d.playlist = { ...d.playlist, track_ids: trackIds };
    d.tracks = optimistic;
    syncItemTrackIds(d.playlist.id, trackIds);
    try {
      const updated = await setPlaylistTracks(d.playlist.id, {
        track_ids: trackIds,
        mode: "replace",
      });
      d.playlist = updated;
      d.tracks = await lib.ensureTracks(updated.track_ids);
      syncItemTrackIds(updated.id, updated.track_ids);
    } catch (e) {
      // Roll back to the pre-mutation state.
      d.playlist = { ...d.playlist, track_ids: prevIds };
      d.tracks = prevTracks;
      syncItemTrackIds(d.playlist.id, prevIds);
      throw e;
    }
  }

  /** Remove one track from the open playlist (optimistic, rollback). */
  async function removeTrack(trackId: number): Promise<void> {
    const d = detail.value;
    if (!d) return;
    const idx = d.playlist.track_ids.indexOf(trackId);
    if (idx < 0) return;
    const next = d.playlist.track_ids.filter((id) => id !== trackId);
    await replaceTracks(next);
  }

  /** Append tracks to a playlist (used by "add to playlist" actions). */
  async function addTracks(id: number, trackIds: number[]): Promise<void> {
    const updated = await setPlaylistTracks(id, { track_ids: trackIds, mode: "append" });
    syncItemTrackIds(id, updated.track_ids);
    if (detail.value?.playlist.id === id) {
      const lib = useLibraryStore();
      detail.value.playlist = updated;
      detail.value.tracks = await lib.ensureTracks(updated.track_ids);
    }
    toasts.push("success", `Added ${trackIds.length} track${trackIds.length === 1 ? "" : "s"} to “${updated.name}”`);
  }

  /** Add a whole album (server expands in disc/track order). */
  async function addAlbum(id: number, albumId: number): Promise<void> {
    const updated = await setPlaylistTracks(id, { album_ids: [albumId], mode: "append" });
    syncItemTrackIds(id, updated.track_ids);
    if (detail.value?.playlist.id === id) {
      const lib = useLibraryStore();
      detail.value.playlist = updated;
      detail.value.tracks = await lib.ensureTracks(updated.track_ids);
    }
    toasts.push("success", `Added album to “${updated.name}”`);
  }

  function syncItemTrackIds(id: number, trackIds: number[]): void {
    const item = items.value.find((p) => p.id === id);
    if (item) item.track_ids = trackIds;
  }

  /** Save the current queue as a named playlist via from_queue. */
  async function saveQueueAs(name: string): Promise<Playlist> {
    const queue = useQueueStore();
    const playlist = await createPlaylist({
      name,
      from_queue: true,
      queue_track_ids: queue.queueIds,
    });
    items.value.push(playlist);
    return playlist;
  }

  /** Upload an .m3u/.m3u8 file; the result (matched/unmatched) goes to
   *  `lastImport` for the dialog and the new playlist appears in the list. */
  async function importFile(file: File, name?: string): Promise<ImportPlaylistResult> {
    const result = await importPlaylistFile(file, name);
    const playlist = await fetchPlaylist(result.playlist_id);
    items.value.push(playlist);
    lastImport.value = {
      name: name || file.name.replace(/\.(m3u8?)$/i, ""),
      result,
    };
    return result;
  }

  function clearImport(): void {
    lastImport.value = null;
  }

  return {
    items,
    detail,
    loading,
    error,
    lastImport,
    loaded,
    load,
    reload,
    open,
    create,
    rename,
    remove,
    replaceTracks,
    removeTrack,
    addTracks,
    addAlbum,
    saveQueueAs,
    importFile,
    clearImport,
  };
});
