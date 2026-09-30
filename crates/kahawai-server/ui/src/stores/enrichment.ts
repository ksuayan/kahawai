import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { setupEnrichmentAction, setupEnrichmentStatus, setupSetEnrichment } from "../tauri";
import type { EnrichAction, EnrichmentStatus } from "../types";
import { isJobActive } from "../types";

/**
 * Settings → Album info: online lookup at MusicBrainz / Cover Art Archive.
 * Off by default (it sends album and artist names off the LAN). Polls while
 * a lookup is running so progress and a self-pause (offline) show up.
 */
export const useEnrichmentStore = defineStore("enrichment", () => {
  const status = ref<EnrichmentStatus | null>(null);
  const busy = ref(false);
  const error = ref<string | null>(null);
  let timer: number | undefined;

  const job = computed(() => status.value?.job ?? null);
  const running = computed(() => job.value != null && isJobActive(job.value));
  const paused = computed(() => job.value?.status === "paused");

  async function load(): Promise<void> {
    status.value = (await setupEnrichmentStatus()) ?? null;
    if (running.value) ensurePolling();
  }

  function ensurePolling(): void {
    if (timer !== undefined) return;
    timer = window.setInterval(async () => {
      await load();
      if (!running.value) {
        window.clearInterval(timer);
        timer = undefined;
      }
    }, 1000);
  }

  async function run(f: () => Promise<void>): Promise<void> {
    error.value = null;
    busy.value = true;
    try {
      await f();
    } catch (err) {
      error.value = String(err);
    } finally {
      busy.value = false;
    }
    await load();
  }

  function setEnabled(enabled: boolean): Promise<void> {
    const threshold = status.value?.min_confidence ?? 0.9;
    return run(() => setupSetEnrichment(enabled, threshold));
  }

  function setThreshold(minConfidence: number): Promise<void> {
    const enabled = status.value?.enabled ?? false;
    return run(() => setupSetEnrichment(enabled, minConfidence));
  }

  function act(action: EnrichAction): Promise<void> {
    return run(() => setupEnrichmentAction(action, job.value?.id));
  }

  return { status, busy, error, job, running, paused, load, setEnabled, setThreshold, act };
});
