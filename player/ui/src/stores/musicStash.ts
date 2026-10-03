import { defineStore } from "pinia";
import { ref } from "vue";
import type { Track } from "../types";

/** The music queue as it was when a book or a podcast took over. */
export interface MusicStash {
  tracks: Track[];
  index: number;
  positionMs: number;
  repeat: "off" | "all" | "one";
  shuffle: boolean;
  wasPlaying: boolean;
}

/**
 * The music put aside while spoken word plays. One place for it, shared by the
 * audiobook and podcast contexts: going from a book to a podcast (or back)
 * hands over to the other context and keeps the music, and Back to music in
 * either brings it back exactly.
 */
export const useMusicStashStore = defineStore("musicStash", () => {
  const stash = ref<MusicStash | null>(null);
  return { stash };
});
