import { defineStore } from "pinia";
import { ref } from "vue";
import { setBaseUrl } from "../api";
import {
  getPlaybackPrefs,
  getServerUrl,
  setBitPerfect,
  setDsdStory,
  setQualityMode,
  setFormat,
  setServerUrl,
} from "../tauri";
import type { BitPerfectMode, DsdStory, QualityMode, StreamFormat } from "../types";

export const DEFAULT_SERVER_URL = "http://localhost:8080";

export const useSettingsStore = defineStore("settings", () => {
  const serverUrl = ref(DEFAULT_SERVER_URL);
  const globalFormat = ref<StreamFormat | null>(null);
  /** DSD handling: "auto" (native on known DoP DACs), "native" or "convert". */
  const dsdStory = ref<DsdStory>("auto");
  /** Top-level quality mode: the Auto settings below follow it. */
  const qualityMode = ref<QualityMode>("best");
  /** Exclusive bit-perfect output: auto (follows the quality mode, default) | off | mqa | all. */
  const bitPerfect = ref<BitPerfectMode>("auto");
  const loaded = ref(false);

  /** On boot: server URL + persisted playback prefs from the Rust core. */
  async function init(): Promise<void> {
    const url = await getServerUrl();
    if (url && url.trim()) {
      serverUrl.value = url.trim();
      setBaseUrl(url.trim());
    }
    const prefs = await getPlaybackPrefs();
    if (prefs) {
      dsdStory.value = prefs.dsd_story;
      globalFormat.value = prefs.global_format;
      bitPerfect.value = prefs.bit_perfect ?? "auto";
      qualityMode.value = prefs.quality_mode ?? "best";
    }
    loaded.value = true;
  }

  /** Persist a new URL through the core, then re-point the REST client. */
  async function saveServerUrl(url: string): Promise<void> {
    const trimmed = url.trim().replace(/\/+$/, "") || DEFAULT_SERVER_URL;
    await setServerUrl(trimmed);
    serverUrl.value = trimmed;
    setBaseUrl(trimmed);
  }

  async function saveGlobalFormat(fmt: StreamFormat | null): Promise<void> {
    globalFormat.value = fmt;
    await setFormat(fmt);
  }

  async function saveQualityMode(mode: QualityMode): Promise<void> {
    qualityMode.value = mode;
    await setQualityMode(mode);
  }

  async function saveDsdStory(story: DsdStory): Promise<void> {
    dsdStory.value = story;
    await setDsdStory(story);
  }

  async function saveBitPerfect(mode: BitPerfectMode): Promise<void> {
    bitPerfect.value = mode;
    await setBitPerfect(mode);
  }

  return {
    serverUrl,
    globalFormat,
    dsdStory,
    qualityMode,
    bitPerfect,
    loaded,
    init,
    saveServerUrl,
    saveGlobalFormat,
    saveDsdStory,
    saveQualityMode,
    saveBitPerfect,
  };
});
