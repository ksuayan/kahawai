import { defineStore } from "pinia";
import { ref } from "vue";
import type { Album, Track } from "../types";

/** What the Info dialog shows. */
export type InfoSubject = { kind: "track"; track: Track } | { kind: "album"; album: Album };

/** Visibility of app-level overlays that any part of the UI may open. */
export const useOverlaysStore = defineStore("overlays", () => {
  /** About dialog (sidebar button, or the app menu's "About Kahawai Player"). */
  const aboutOpen = ref(false);

  function openAbout(): void {
    aboutOpen.value = true;
  }

  /** Info dialog (a track's or an album's details); null when closed. */
  const info = ref<InfoSubject | null>(null);

  function showInfo(subject: InfoSubject): void {
    info.value = subject;
  }

  return { aboutOpen, openAbout, info, showInfo };
});
