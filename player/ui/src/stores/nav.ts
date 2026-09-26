import { defineStore } from "pinia";
import { ref } from "vue";

export type ViewName =
  | "albums"
  | "artists"
  | "playlists"
  | "search"
  | "queue"
  | "settings";

export interface NavState {
  name: ViewName | "album" | "artist" | "playlist" | "nowplaying";
  id?: number;
}

/** Minimal router: the app is a single Tauri window with view state. */
export const useNavStore = defineStore("nav", () => {
  const view = ref<NavState>({ name: "albums" });

  function go(name: NavState["name"], id?: number): void {
    view.value = { name, id };
  }

  return { view, go };
});
