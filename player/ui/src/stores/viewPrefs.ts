import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { isSortKey, isTrackSortKey, type SortKey, type TrackSortKey } from "../lib/sorting";
import { isRadioSort, type RadioSort } from "../lib/radioSort";
import { uiGet, uiSet } from "../lib/uiState";

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
  genreTracksLayout: LayoutMode;
  queueLayout: LayoutMode;
  queueSort: TrackSortKey;
  albumTracksLayout: LayoutMode;
  playlistTracksLayout: LayoutMode;
  albumTracksSort: TrackSortKey;
  searchLayout: LayoutMode;
  searchSort: TrackSortKey;
  audiobooksLayout: LayoutMode;
  podcastsLayout: LayoutMode;
  radioLayout: LayoutMode;
  radioSort: RadioSort;
}

const DEFAULTS: ViewPrefs = {
  albumsLayout: "grid",
  albumsSort: "artist-asc",
  artistsLayout: "list",
  artistsSort: "name-asc",
  genresLayout: "grid",
  genresSort: "count-desc",
  genreTracksSort: "artist-asc",
  genreTracksLayout: "list",
  queueLayout: "list",
  queueSort: "default",
  albumTracksLayout: "list",
  playlistTracksLayout: "list",
  albumTracksSort: "default",
  searchLayout: "list",
  searchSort: "default",
  audiobooksLayout: "grid",
  podcastsLayout: "grid",
  radioLayout: "list",
  radioSort: "listed",
};

const KEY = "kahawai.viewPrefs";
const layouts = ["list", "grid"];

/** Stored prefs, keeping only values this version understands. */
function load(): ViewPrefs {
  const prefs = { ...DEFAULTS };
  try {
    const raw = JSON.parse(uiGet(KEY) ?? "{}") as Record<string, unknown>;
    for (const k of [
      "albumsLayout",
      "artistsLayout",
      "genresLayout",
      "genreTracksLayout",
      "queueLayout",
      "albumTracksLayout",
      "playlistTracksLayout",
      "searchLayout",
      "audiobooksLayout",
      "podcastsLayout",
      "radioLayout",
    ] as const) {
      if (layouts.includes(raw[k] as string)) prefs[k] = raw[k] as LayoutMode;
    }
    if (isSortKey(raw.albumsSort)) prefs.albumsSort = raw.albumsSort;
    if (isRadioSort(raw.radioSort)) prefs.radioSort = raw.radioSort;
    if (isSortKey(raw.genreTracksSort)) prefs.genreTracksSort = raw.genreTracksSort;
    for (const k of ["queueSort", "albumTracksSort", "searchSort"] as const) {
      if (isTrackSortKey(raw[k])) prefs[k] = raw[k] as TrackSortKey;
    }
    if (raw.artistsSort === "name-asc" || raw.artistsSort === "name-desc") prefs.artistsSort = raw.artistsSort;
    if (["count-desc", "count-asc", "name-asc", "name-desc"].includes(raw.genresSort as string)) {
      prefs.genresSort = raw.genresSort as ViewPrefs["genresSort"];
    }
  } catch {
    // Unreadable or blocked storage: defaults.
  }
  return prefs;
}

/** Remembered per view across launches (ui-state.json; see lib/uiState). */
export const useViewPrefsStore = defineStore("viewPrefs", () => {
  const prefs = ref<ViewPrefs>(load());
  watch(
    prefs,
    (p) => {
      try {
        uiSet(KEY, JSON.stringify(p));
      } catch {
        // Storage unavailable: the choice lasts this session.
      }
    },
    { deep: true },
  );
  return { prefs };
});
