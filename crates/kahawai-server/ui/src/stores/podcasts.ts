import { defineStore } from "pinia";
import { ref } from "vue";
import { setupPickDirectory, setupPodcastSettings, setupSetPodcastSettings } from "../tauri";
import type { PodcastSettings } from "../types";

/** Settings → Podcasts: where episodes are downloaded to, and how often feeds are checked. */
export const usePodcastsStore = defineStore("podcasts", () => {
  const settings = ref<PodcastSettings | null>(null);
  const busy = ref(false);
  const error = ref<string | null>(null);

  async function load(): Promise<void> {
    settings.value = (await setupPodcastSettings()) ?? null;
  }

  async function save(dir: string | null, hours: number): Promise<void> {
    error.value = null;
    busy.value = true;
    try {
      await setupSetPodcastSettings(dir, hours);
    } catch (err) {
      error.value = String(err);
    } finally {
      busy.value = false;
    }
    await load();
  }

  /** Pick a folder; downloads already made stay where they are. */
  async function chooseFolder(): Promise<void> {
    const picked = await setupPickDirectory();
    if (!picked || !settings.value) return;
    await save(picked, settings.value.refresh_hours);
  }

  function useDefaultFolder(): Promise<void> {
    return save(null, settings.value?.refresh_hours ?? 6);
  }

  function setRefreshHours(hours: number): Promise<void> {
    return save(settings.value?.custom ? settings.value.path : null, hours);
  }

  return { settings, busy, error, load, chooseFolder, useDefaultFolder, setRefreshHours };
});
