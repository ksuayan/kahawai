import { defineStore } from "pinia";
import { ref } from "vue";
import { inTauri, outputLiveState, type OutputLive } from "../tauri";

/**
 * The output device's real, current state (rate, stream format, exclusive
 * ownership), read from the OS. Polled only while something is watching
 * (the Settings signal-path panel), so it costs nothing the rest of the time.
 */
export const useSignalStore = defineStore("signal", () => {
  const live = ref<OutputLive | null>(null);
  let timer: number | undefined;
  let watchers = 0;

  async function refresh(): Promise<void> {
    if (!inTauri()) return;
    live.value = (await outputLiveState()) ?? null;
  }

  /** Begin polling (reference-counted). Returns the stop function. */
  function watch(intervalMs = 1000): () => void {
    watchers += 1;
    void refresh();
    if (timer === undefined) timer = window.setInterval(() => void refresh(), intervalMs);
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      watchers -= 1;
      if (watchers <= 0 && timer !== undefined) {
        window.clearInterval(timer);
        timer = undefined;
        watchers = 0;
      }
    };
  }

  return { live, refresh, watch };
});
