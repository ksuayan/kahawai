import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { isSortKey, type SortKey } from "../lib/sorting";

export type LayoutMode = "list" | "grid";

/** How each list view is shown: list or grid, and its sort order. */
export interface ViewPrefs {
  albumsLayout: LayoutMode;
  albumsSort: SortKey;
  artistsLayout: LayoutMode;
  artistsSort: "name-asc" | "name-desc";
  genresLayout: LayoutMode;
  genresSort: "count-desc" | "count-asc" | "name-asc" | "name-desc";
  genreTracksSort: SortKey;
}

const DEFAULTS: ViewPrefs = {
  albumsLayout: "grid",
  albumsSort: "artist-asc",
  artistsLayout: "list",
  artistsSort: "name-asc",
  genresLayout: "grid",
  genresSort: "count-desc",
  genreTracksSort: "artist-asc",
};

const KEY = "kahawai.viewPrefs";
const layouts = ["list", "grid"];

/** Stored prefs, keeping only values this version understands. */
function load(): ViewPrefs {
  const prefs = { ...DEFAULTS };
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, unknown>;
    for (const k of ["albumsLayout", "artistsLayout", "genresLayout"] as const) {
      if (layouts.includes(raw[k] as string)) prefs[k] = raw[k] as LayoutMode;
    }
    if (isSortKey(raw.albumsSort)) prefs.albumsSort = raw.albumsSort;
    if (isSortKey(raw.genreTracksSort)) prefs.genreTracksSort = raw.genreTracksSort;
    if (raw.artistsSort === "name-asc" || raw.artistsSort === "name-desc") prefs.artistsSort = raw.artistsSort;
    if (["count-desc", "count-asc", "name-asc", "name-desc"].includes(raw.genresSort as string)) {
      prefs.genresSort = raw.genresSort as ViewPrefs["genresSort"];
    }
  } catch {
    // Unreadable or blocked storage: defaults.
  }
  return prefs;
}

/** Remembered per view across launches (a viewer convenience: localStorage). */
export const useViewPrefsStore = defineStore("viewPrefs", () => {
  const prefs = ref<ViewPrefs>(load());
  watch(
    prefs,
    (p) => {
      try {
        localStorage.setItem(KEY, JSON.stringify(p));
      } catch {
        // Storage unavailable: the choice lasts this session.
      }
    },
    { deep: true },
  );
  return { prefs };
});
