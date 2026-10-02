import { defineStore } from "pinia";
import { ref } from "vue";
import { uiGet, uiSet } from "../lib/uiState";

export type ViewName =
  | "albums"
  | "artists"
  | "genres"
  | "playlists"
  | "audiobooks"
  | "search"
  | "queue"
  | "settings";

export interface NavState {
  name: ViewName | "album" | "artist" | "playlist" | "genre" | "audiobook" | "nowplaying";
  id?: number;
  /** The genre a "genre" view shows (genres are named, not numbered). */
  genre?: string;
}

const KEY = "kahawai.nav";
const LISTS = ["albums", "artists", "genres", "playlists", "audiobooks", "search", "queue", "settings", "nowplaying"];
const BY_ID = ["album", "artist", "playlist", "audiobook"];

/** The view the app was on when it quit, if it still makes sense. */
function lastView(): NavState {
  try {
    const v = JSON.parse(uiGet(KEY) ?? "null") as Partial<NavState> | null;
    if (v && typeof v.name === "string") {
      if (LISTS.includes(v.name)) return { name: v.name };
      if (BY_ID.includes(v.name) && Number.isInteger(v.id)) return { name: v.name, id: v.id };
      if (v.name === "genre" && typeof v.genre === "string" && v.genre) return { name: "genre", genre: v.genre };
    }
  } catch {
    // Unreadable: start on Albums.
  }
  return { name: "albums" };
}

/** Minimal router: the app is a single Tauri window with view state. The
 *  view is saved on every change, and the app reopens on it. */
export const useNavStore = defineStore("nav", () => {
  const view = ref<NavState>(lastView());

  function go(name: NavState["name"], id?: number, genre?: string): void {
    view.value = genre === undefined ? { name, id } : { name, id, genre };
    uiSet(KEY, JSON.stringify(view.value));
  }

  return { view, go };
});
