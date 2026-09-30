import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import { isPlayable, type Album, type Artist, type Track } from "../types";

/** "Taylor Swift feat. Post Malone" → "Taylor Swift": the lead artist of a
 *  credit, for finding the artist page when the full credit has none. */
function leadArtist(name: string): string {
  return name.split(/\s+(?:feat\.?|ft\.?|featuring|with|&|x)\s+|\s*[,;/]\s*/i)[0].trim();
}

const plural = (n: number) => `${n} track${n === 1 ? "" : "s"}`;

/**
 * What the item menus do (the right-click menu on rows and cards, and the
 * ⋯ menu): play, go to an item's artist or album, add to the queue or a
 * playlist without adding anything twice, and show Info.
 */
export function useLibraryActions() {
  const lib = useLibraryStore();
  const nav = useNavStore();
  const queue = useQueueStore();
  const playlists = usePlaylistsStore();
  const toasts = useToastsStore();

  /** The artist page for a credit: the exact name, else its lead artist. */
  function findArtist(name: string | null | undefined): Artist | undefined {
    if (!name) return undefined;
    const find = (n: string) => {
      const key = n.trim().toLowerCase();
      return lib.artists.find((a) => a.name.trim().toLowerCase() === key);
    };
    return find(name) ?? find(leadArtist(name));
  }

  function goToArtist(name: string | null | undefined): void {
    const artist = findArtist(name);
    if (artist) nav.go("artist", artist.id);
  }

  function goToAlbum(id: number | null | undefined): void {
    if (id != null) nav.go("album", id);
  }

  /** An album's tracks in play order (the cache when offline). */
  async function albumTracks(album: Album): Promise<Track[]> {
    return (await lib.getAlbumDetail(album.id)).tracks;
  }

  async function playAlbum(album: Album): Promise<void> {
    const tracks = (await albumTracks(album)).filter(isPlayable);
    if (tracks.length > 0) await queue.playAll(tracks, 0);
  }

  /** Distinct playable tracks: the count "already there" is measured against. */
  const distinctPlayable = (tracks: Track[]) => new Set(tracks.filter(isPlayable).map((t) => t.id)).size;

  /** Tracks not already listed, without repeats within `tracks` either. */
  function fresh(tracks: Track[], present: Iterable<number>): Track[] {
    const seen = new Set(present);
    return tracks.filter((t) => isPlayable(t) && !seen.has(t.id) && (seen.add(t.id), true));
  }

  /** Append what isn't in the queue yet. */
  async function addToQueue(tracks: Track[]): Promise<void> {
    const add = fresh(tracks, queue.tracks.map((t) => t.id));
    const skipped = distinctPlayable(tracks) - add.length;
    if (add.length === 0) {
      toasts.push("info", tracks.length === 1 ? "Already in the queue" : "All already in the queue");
      return;
    }
    try {
      await queue.appendTracks(add);
      toasts.push(
        "success",
        `Added ${plural(add.length)} to queue`,
        skipped > 0 ? { detail: `${plural(skipped)} already in the queue` } : undefined,
      );
    } catch (e) {
      toasts.push("error", "Add to queue failed", { detail: e instanceof Error ? e.message : String(e) });
    }
  }

  /** Append what isn't in the playlist yet. */
  async function addToPlaylist(playlistId: number, tracks: Track[]): Promise<void> {
    const playlist = playlists.items.find((p) => p.id === playlistId);
    const add = fresh(tracks, playlist?.track_ids ?? []);
    const skipped = distinctPlayable(tracks) - add.length;
    const name = playlist ? `“${playlist.name}”` : "the playlist";
    if (add.length === 0) {
      toasts.push("info", tracks.length === 1 ? `Already in ${name}` : `All already in ${name}`);
      return;
    }
    try {
      await playlists.addTracks(playlistId, add.map((t) => t.id));
      if (skipped > 0) toasts.push("info", `${plural(skipped)} already in ${name}`);
    } catch (e) {
      toasts.push("error", "Add to playlist failed", { detail: e instanceof Error ? e.message : String(e) });
    }
  }

  async function newPlaylistWith(name: string, tracks: Track[]): Promise<void> {
    try {
      const playlist = await playlists.create(name);
      await addToPlaylist(playlist.id, tracks);
    } catch (e) {
      toasts.push("error", "Could not create playlist", { detail: e instanceof Error ? e.message : String(e) });
    }
  }

  return { findArtist, goToArtist, goToAlbum, albumTracks, playAlbum, addToQueue, addToPlaylist, newPlaylistWith };
}
