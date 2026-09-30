import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { makeTrack, mockFetch } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useScrollMemoryStore } from "../stores/scrollMemory";
import { useViewPrefsStore } from "../stores/viewPrefs";
import SearchView from "../components/SearchView.vue";
import { loadUiState, uiGet, uiSet } from "./uiState";

const inApp = () => ((window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {});

/** A fresh launch: new stores over whatever was saved. */
const relaunch = () => setActivePinia(createPinia());

beforeEach(() => relaunch());

describe("the UI state file", () => {
  it("in the app, reads the shell's file and writes every change through", async () => {
    inApp();
    tauri.on("get_ui_state", { "kahawai.viewPrefs": JSON.stringify({ albumsLayout: "list" }) });
    await loadUiState();
    expect(useViewPrefsStore().prefs.albumsLayout).toBe("list");
    uiSet("kahawai-player.theme", "dark");
    expect(tauri.callsTo("set_ui_state")).toEqual([{ key: "kahawai-player.theme", value: "dark" }]);
  });

  it("moves a value an earlier version kept in localStorage into the file", async () => {
    localStorage.setItem("kahawai-player.eq-rows", "[1]");
    inApp();
    tauri.on("get_ui_state", {});
    await loadUiState();
    expect(uiGet("kahawai-player.eq-rows")).toBe("[1]");
    expect(tauri.callsTo("set_ui_state")).toEqual([{ key: "kahawai-player.eq-rows", value: "[1]" }]);
  });

  it("outside the app (plain browser dev), localStorage is the store", () => {
    uiSet("k", "v");
    expect(localStorage.getItem("k")).toBe("v");
    expect(uiGet("k")).toBe("v");
    expect(tauri.callsTo("set_ui_state")).toHaveLength(0);
  });
});

describe("picking up where you left off", () => {
  it("reopens the last view, including which album or genre", () => {
    useNavStore().go("album", 42);
    relaunch();
    expect(useNavStore().view).toEqual({ name: "album", id: 42 });
    useNavStore().go("genre", undefined, "R&B");
    relaunch();
    expect(useNavStore().view).toEqual({ name: "genre", genre: "R&B" });
  });

  it("starts on Albums when the saved view doesn't make sense", () => {
    for (const bad of ["not json", JSON.stringify({ name: "album" }), JSON.stringify({ name: "nowhere" })]) {
      localStorage.setItem("kahawai.nav", bad);
      relaunch();
      expect(useNavStore().view).toEqual({ name: "albums" });
    }
  });

  it("remembers scroll positions", () => {
    useScrollMemoryStore().set("albums", 4200);
    relaunch();
    expect(useScrollMemoryStore().get("albums")).toBe(4200);
  });

  it("remembers the last search and runs it again on Search", async () => {
    useLibraryStore().search("coltrane");
    relaunch();
    expect(useLibraryStore().searchQuery).toBe("coltrane");
    const calls = mockFetch({ "/api/search": [makeTrack({ title: "Naima" })] });
    const { wrapper } = mountApp(SearchView);
    await new Promise((r) => setTimeout(r, 300)); // the search's debounce
    await settle();
    expect(calls.some((c) => c.url.includes("/api/search?q=coltrane"))).toBe(true);
    expect(wrapper.text()).toContain("Naima");
  });
});
