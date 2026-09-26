import { defineStore } from "pinia";
import { ref } from "vue";

/** Visibility of app-level overlays that any part of the UI may open. */
export const useOverlaysStore = defineStore("overlays", () => {
  /** About dialog (sidebar button, or the app menu's "About Kahawai Player"). */
  const aboutOpen = ref(false);

  function openAbout(): void {
    aboutOpen.value = true;
  }

  return { aboutOpen, openAbout };
});
