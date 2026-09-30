import { defineStore } from "pinia";
import { ref } from "vue";

export type ViewName =
  | "albums"
  | "artists"
  | "genres"
  | "playlists"
  | "search"
  | "queue"
  | "settings";

export interface NavState {
  name: ViewName | "album" | "artist" | "playlist" | "genre" | "nowplaying";
  id?: number;
  /** The genre a "genre" view shows (genres are named, not numbered). */
  genre?: string;
}

/** Minimal router: the app is a single Tauri window with view state. */
export const useNavStore = defineStore("nav", () => {
  const view = ref<NavState>({ name: "albums" });

  function go(name: NavState["name"], id?: number, genre?: string): void {
    view.value = genre === undefined ? { name, id } : { name, id, genre };
  }

  return { view, go };
});
