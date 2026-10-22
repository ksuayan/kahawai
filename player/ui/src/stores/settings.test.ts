import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { getAuthToken, getBaseUrl, setAuthToken, setBaseUrl } from "../api";
import { tauri } from "../test/tauri-mock";
import { DEFAULT_SERVER_URL, useSettingsStore } from "./settings";

beforeEach(() => {
  setActivePinia(createPinia());
  setAuthToken(null);
  setBaseUrl("http://localhost:8080");
});

describe("settings store", () => {
  it("boots with the saved server URL and playback prefs, and points the REST client at it", async () => {
    tauri
      .on("get_server_url", "  http://10.0.0.233:8080  ")
      .on("get_playback_prefs", { dsd_story: "native", global_format: "flac" });
    const s = useSettingsStore();
    await s.init();
    expect(s.serverUrl).toBe("http://10.0.0.233:8080");
    expect(getBaseUrl()).toBe("http://10.0.0.233:8080");
    expect(s.dsdStory).toBe("native");
    expect(s.globalFormat).toBe("flac");
    expect(s.loaded).toBe(true);
  });

  it("still boots (with defaults) when the core is unavailable", async () => {
    const s = useSettingsStore();
    await s.init();
    expect(s.loaded).toBe(true);
    expect(s.serverUrl).toBe(DEFAULT_SERVER_URL);
    expect(s.dsdStory).toBe("auto");
    expect(s.globalFormat).toBeNull();
  });

  it("normalises and persists a new server URL", async () => {
    const s = useSettingsStore();
    await s.saveServerUrl("  http://nas:8080///  ");
    expect(s.serverUrl).toBe("http://nas:8080");
    expect(getBaseUrl()).toBe("http://nas:8080");
    expect(tauri.callsTo("set_server_url")).toEqual([{ url: "http://nas:8080" }]);
    await s.saveServerUrl("   ");
    expect(s.serverUrl).toBe(DEFAULT_SERVER_URL);
  });

  it("persists DSD handling and the global format through the core", async () => {
    const s = useSettingsStore();
    await s.saveDsdStory("native");
    await s.saveGlobalFormat("opus");
    await s.saveGlobalFormat(null);
    expect(s.dsdStory).toBe("native");
    expect(s.globalFormat).toBeNull();
    expect(tauri.callsTo("set_dsd_story")).toEqual([{ story: "native" }]);
    expect(tauri.callsTo("set_format").length).toBe(2);
  });

  it("bit-perfect defaults to auto, restores the saved mode, and saves changes through the core", async () => {
    const auto = useSettingsStore();
    await auto.init();
    expect(auto.bitPerfect).toBe("auto");
    expect(auto.qualityMode).toBe("best");

    setActivePinia(createPinia());
    tauri.on("get_playback_prefs", { dsd_story: "convert", global_format: null, bit_perfect: "mqa" });
    const s = useSettingsStore();
    await s.init();
    expect(s.bitPerfect).toBe("mqa");

    await s.saveBitPerfect("all");
    expect(s.bitPerfect).toBe("all");
    expect(tauri.callsTo("set_bit_perfect")).toEqual([{ mode: "all" }]);
    await s.saveBitPerfect("off");
    expect(tauri.callsTo("set_bit_perfect").at(-1)).toEqual({ mode: "off" });
  });

  it("an older core without the setting follows the quality mode", async () => {
    tauri.on("get_playback_prefs", { dsd_story: "native", global_format: "flac" });
    const s = useSettingsStore();
    await s.init();
    expect(s.bitPerfect).toBe("auto");
  });

  it("restores and saves the quality mode through the core", async () => {
    tauri.on("get_playback_prefs", { dsd_story: "auto", global_format: null, bit_perfect: "auto", quality_mode: "compatible" });
    const s = useSettingsStore();
    await s.init();
    expect(s.qualityMode).toBe("compatible");
    await s.saveQualityMode("best");
    expect(s.qualityMode).toBe("best");
    expect(tauri.callsTo("set_quality_mode")).toEqual([{ mode: "best" }]);
  });
});

describe("settings store: server credential (P0 §1.3)", () => {
  it("restores the keychain token on init", async () => {
    tauri.on("get_server_token", "keychain-token");
    const s = useSettingsStore();
    await s.init();
    expect(s.serverToken).toBe("keychain-token");
    expect(getAuthToken()).toBe("keychain-token");
  });

  it("boots logged out when the keychain is empty", async () => {
    tauri.on("get_server_token", null);
    const s = useSettingsStore();
    await s.init();
    expect(s.serverToken).toBeNull();
    expect(getAuthToken()).toBeNull();
  });

  it("saveServerToken persists to the keychain and arms the API client", async () => {
    tauri.on("set_server_token", undefined);
    const s = useSettingsStore();
    await s.saveServerToken("  pair-123  ");
    expect(s.serverToken).toBe("pair-123");
    expect(s.tokenKind).toBe("pairing");
    expect(getAuthToken()).toBe("pair-123");
    expect(tauri.callsTo("set_server_token")).toEqual([{ token: "pair-123" }]);
  });

  it("logout revokes server-side and forgets the credential", async () => {
    tauri.on("set_server_token", undefined).on("clear_server_token", undefined);
    const s = useSettingsStore();
    await s.saveServerToken("pair-123");
    // Mock the /api/logout POST via fetch.
    const origFetch = globalThis.fetch;
    (globalThis as unknown as { fetch: unknown }).fetch = async () =>
      new Response(JSON.stringify({}), { status: 200 });
    try {
      await s.logout();
    } finally {
      (globalThis as unknown as { fetch: unknown }).fetch = origFetch;
    }
    expect(s.serverToken).toBeNull();
    expect(getAuthToken()).toBeNull();
    expect(tauri.callsTo("clear_server_token")).toHaveLength(1);
  });
});
