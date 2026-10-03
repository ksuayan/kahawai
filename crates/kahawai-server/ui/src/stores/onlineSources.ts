import { defineStore } from "pinia";
import { ref } from "vue";
import { setupOnlineSources, setupSetOnlineSources } from "../tauri";

/**
 * Settings → Online sources: the internet radio station directory
 * (radio-browser.info) and the podcast directory (Apple's iTunes Search).
 * Off by default: what you search for is sent to those services. Saved
 * stations and feeds, and ones added by address, work without it.
 */
export const useOnlineSourcesStore = defineStore("onlineSources", () => {
  /** null until the running server has answered. */
  const enabled = ref<boolean | null>(null);
  const busy = ref(false);
  const error = ref<string | null>(null);

  async function load(): Promise<void> {
    enabled.value = (await setupOnlineSources()) ?? null;
  }

  async function setEnabled(on: boolean): Promise<void> {
    error.value = null;
    busy.value = true;
    try {
      await setupSetOnlineSources(on);
    } catch (err) {
      error.value = String(err);
    } finally {
      busy.value = false;
    }
    await load();
  }

  return { enabled, busy, error, load, setEnabled };
});
