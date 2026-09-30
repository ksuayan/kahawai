import { defineStore } from "pinia";
import { ref } from "vue";

/** Remembers how far each scrolling view was scrolled, so coming back to it
 *  (the views unmount when you navigate away) puts the user where they were.
 *  Kept in memory only: a fresh launch starts at the top. */
export const useScrollMemoryStore = defineStore("scrollMemory", () => {
  const offsets = ref<Record<string, number>>({});

  /** The remembered scroll offset (px) for `key`, 0 when none. */
  function get(key: string): number {
    return offsets.value[key] ?? 0;
  }

  function set(key: string, px: number): void {
    if (Number.isFinite(px) && px > 0) offsets.value[key] = Math.round(px);
    else delete offsets.value[key];
  }

  return { get, set };
});
