import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { getBaseUrl } from "../api";
import { tauri } from "../test/tauri-mock";
import { DEFAULT_SERVER_URL, useSettingsStore } from "./settings";

beforeEach(() => setActivePinia(createPinia()));

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
    expect(s.dsdStory).toBe("convert");
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

  it("bit-perfect defaults to off, restores the saved mode, and saves changes through the core", async () => {
    const off = useSettingsStore();
    await off.init();
    expect(off.bitPerfect).toBe("off");

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

  it("an older core without the setting leaves it off", async () => {
    tauri.on("get_playback_prefs", { dsd_story: "native", global_format: "flac" });
    const s = useSettingsStore();
    await s.init();
    expect(s.bitPerfect).toBe("off");
  });
});
