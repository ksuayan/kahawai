/**
 * Sort orders for the list views' Sort dropdown. Albums (and a genre's
 * tracks, sorted server-side) sort by artist, album title or release year,
 * each either way. Unknown years sort last in both directions: "newest
 * first" shouldn't open on a wall of undated albums.
 */
import type { Album } from "../types";

export type SortField = "artist" | "title" | "year";
export type SortDir = "asc" | "desc";
/** "artist-asc", "year-desc", … */
export type SortKey = `${SortField}-${SortDir}`;

export const SORT_OPTIONS: { value: SortKey; label: string }[] = [
  { value: "artist-asc", label: "Artist (A–Z)" },
  { value: "artist-desc", label: "Artist (Z–A)" },
  { value: "title-asc", label: "Album title (A–Z)" },
  { value: "title-desc", label: "Album title (Z–A)" },
  { value: "year-asc", label: "Release year (oldest first)" },
  { value: "year-desc", label: "Release year (newest first)" },
];

export function parseSort(key: SortKey): { field: SortField; dir: SortDir } {
  const [field, dir] = key.split("-") as [SortField, SortDir];
  return { field, dir };
}

export function isSortKey(v: unknown): v is SortKey {
  return SORT_OPTIONS.some((o) => o.value === v);
}

const text = (s: string | null | undefined) => (s ?? "").trim();
const cmpText = (a: string, b: string) =>
  a.localeCompare(b, undefined, { sensitivity: "base", numeric: true });

/** Albums in the given order. Ties fall back to artist, year, then title,
 *  using the server's sort keys ("Beatles, The") when it sends them. */
export function sortAlbums(albums: Album[], key: SortKey): Album[] {
  const { field, dir } = parseSort(key);
  const sign = dir === "asc" ? 1 : -1;
  const artist = (a: Album) => text(a.sort_artist ?? a.artist);
  const title = (a: Album) => text(a.sort_title ?? a.title);
  const byArtist = (a: Album, b: Album) => cmpText(artist(a), artist(b));
  const byTitle = (a: Album, b: Album) => cmpText(title(a), title(b));
  const byYear = (a: Album, b: Album) => (a.year ?? 0) - (b.year ?? 0);
  return [...albums].sort((a, b) => {
    if (field === "year") {
      const ay = a.year ?? null;
      const by = b.year ?? null;
      if (ay === null || by === null) {
        if (ay !== by) return ay === null ? 1 : -1; // unknown years last
      } else if (ay !== by) return sign * (ay - by);
      return byArtist(a, b) || byTitle(a, b);
    }
    if (field === "title") return sign * byTitle(a, b) || byArtist(a, b);
    return sign * byArtist(a, b) || byYear(a, b) || byTitle(a, b);
  });
}
