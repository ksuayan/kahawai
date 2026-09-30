import { defineStore } from "pinia";
import { ref } from "vue";
import { uiGet, uiSet } from "../lib/uiState";

const KEY = "kahawai.scroll";

function saved(): Record<string, number> {
  try {
    const v = JSON.parse(uiGet(KEY) ?? "{}") as Record<string, unknown>;
    return Object.fromEntries(
      Object.entries(v).filter((e): e is [string, number] => Number.isFinite(e[1]) && (e[1] as number) > 0),
    );
  } catch {
    return {};
  }
}


/** Remembers how far each scrolling view was scrolled, so coming back to it
 *  (the views unmount when you navigate away) puts the user where they were.
 *  Saved too, so a restart reopens the last view where it was scrolled to
 *  (the views save while scrolling, throttled: see lib/ownScroll). */
export const useScrollMemoryStore = defineStore("scrollMemory", () => {
  const offsets = ref<Record<string, number>>(saved());

  /** The remembered scroll offset (px) for `key`, 0 when none. */
  function get(key: string): number {
    return offsets.value[key] ?? 0;
  }

  function set(key: string, px: number): void {
    if (Number.isFinite(px) && px > 0) offsets.value[key] = Math.round(px);
    else delete offsets.value[key];
    uiSet(KEY, JSON.stringify(offsets.value));
  }

  return { get, set };
});
