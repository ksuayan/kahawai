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

  /** Server sign-in dialog (P0 §1.3 + Phase 3 §3.3). */
  const authOpen = ref(false);
  /** Why it opened: an expired session, a dead pairing token, or manual. */
  const authReason = ref<"expired" | "token-changed" | "manual">("manual");

  function openAuth(reason: "expired" | "token-changed" | "manual"): void {
    authReason.value = reason;
    authOpen.value = true;
  }

  return { aboutOpen, openAbout, info, showInfo, authOpen, authReason, openAuth };
});
