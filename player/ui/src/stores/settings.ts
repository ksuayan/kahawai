import { defineStore } from "pinia";
import { ref } from "vue";
import { login as apiLogin, logout as apiLogout, setAuthToken, setBaseUrl } from "../api";
import {
  clearServerToken,
  getPlaybackPrefs,
  getServerToken,
  getServerUrl,
  setBitPerfect,
  setDsdStory,
  setQualityMode,
  setFormat,
  setServerToken,
  setServerUrl,
} from "../tauri";
import type { BitPerfectMode, DsdStory, QualityMode, StreamFormat } from "../types";

export const DEFAULT_SERVER_URL = "http://localhost:8080";

export const useSettingsStore = defineStore("settings", () => {
  const serverUrl = ref(DEFAULT_SERVER_URL);
  /** The server credential (pairing or session token). Mirrors the OS
   *  keychain; `api.ts` sends it as `Authorization: Bearer`. Null = not
   *  paired / logged out. */
  const serverToken = ref<string | null>(null);
  /** How the credential was obtained: drives the 401 message ("session
   *  expired" vs "server token changed"). */
  const tokenKind = ref<"pairing" | "session" | null>(null);
  const globalFormat = ref<StreamFormat | null>(null);
  /** DSD handling: "auto" (native on known DoP DACs), "native" or "convert". */
  const dsdStory = ref<DsdStory>("auto");
  /** Top-level quality mode: the Auto settings below follow it. */
  const qualityMode = ref<QualityMode>("best");
  /** Exclusive bit-perfect output: auto (follows the quality mode, default) | off | mqa | all. */
  const bitPerfect = ref<BitPerfectMode>("auto");
  const loaded = ref(false);

  /** On boot: server URL + credential from the OS keychain + playback prefs. */
  async function init(): Promise<void> {
    const url = await getServerUrl();
    if (url && url.trim()) {
      serverUrl.value = url.trim();
      setBaseUrl(url.trim());
    }
    // P0 §1.3: the credential lives in the OS keychain, never in settings.
    const token = await getServerToken();
    if (token) {
      serverToken.value = token;
      setAuthToken(token);
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

  /** Pair with a server: persist the pairing token in the OS keychain and
   *  send it on every request from now on (P0 §1.3). */
  async function saveServerToken(token: string): Promise<void> {
    const trimmed = token.trim();
    if (!trimmed) throw new Error("token must not be empty");
    await setServerToken(trimmed);
    serverToken.value = trimmed;
    tokenKind.value = "pairing";
    setAuthToken(trimmed);
  }

  /** Log in with a username + password (Phase 3): the session token
   *  replaces any pairing token in the keychain. */
  async function login(username: string, password: string): Promise<void> {
    const token = await apiLogin(username, password);
    await setServerToken(token);
    serverToken.value = token;
    tokenKind.value = "session";
    setAuthToken(token);
  }

  /** Log out: revoke the session server-side, then forget the credential. */
  async function logout(): Promise<void> {
    try {
      await apiLogout();
    } finally {
      await clearServerToken().catch(() => {});
      serverToken.value = null;
      tokenKind.value = null;
      setAuthToken(null);
    }
  }

  /** Forget the credential without a server round-trip (e.g. after a 401
   *  that says the token is dead). */
  async function forgetCredential(): Promise<void> {
    await clearServerToken().catch(() => {});
    serverToken.value = null;
    tokenKind.value = null;
    setAuthToken(null);
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
    serverToken,
    tokenKind,
    login,
    logout,
    forgetCredential,
    saveServerToken,
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
