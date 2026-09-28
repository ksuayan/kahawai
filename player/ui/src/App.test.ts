import { describe, expect, it } from "vitest";
import App from "./App.vue";
import { makeAlbum, makeState, makeTrack, mockFetch } from "./test/fixtures";
import { key, mountApp, settle, typeInto } from "./test/helpers";
import { tauri } from "./test/tauri-mock";
import { latestEventSource } from "./test/eventsource-mock";
import { useNavStore } from "./stores/nav";
import { useAbxStore } from "./stores/abx";
import { useAnalogStore } from "./stores/analog";
import { useOverlaysStore } from "./stores/overlays";
import { useQueueStore } from "./stores/queue";
import { useToastsStore } from "./stores/toasts";
import { usePlayerStore } from "./stores/player";

const albums = [makeAlbum({ title: "Stranger in the Alps", artist: "Phoebe Bridgers", track_count: 3 })];

function online() {
  return mockFetch({
    "/api/health": { status: "ok" },
    "/api/albums": { items: albums, page: 1, per_page: 500, total: 1 },
    "/api/artists": [{ id: 1, name: "Phoebe Bridgers" }],
    "/api/playlists": [],
    "/api/jobs": [],
  });
}

function tauriApp(state = makeState({ status: "playing", position_ms: 60_000, volume: 0.5, track: makeTrack({ duration_ms: 300_000 }) })) {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  tauri
    .on("get_server_url", "http://server:8080")
    .on("get_state", state)
    .on("get_playback_prefs", { dsd_story: "convert", global_format: null })
    .on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14 })
    .on("get_output_devices", [{ name: "Built-in Output", is_default: true }])
    .on("get_output_device", null)
    .on("dop_status", { supported_rates: [], exclusive_available: true });
}

async function bootApp() {
  const { wrapper } = mountApp(App);
  await settle();
  await settle();
  return wrapper;
}

describe("App: boot", () => {
  it("shows the library once the core, engine state and server have answered", async () => {
    tauriApp();
    online();
    const w = await bootApp();
    expect(w.text()).not.toContain("Starting…");
    expect(w.text()).toContain("Stranger in the Alps");
    expect(w.find('[data-testid="now-playing-bar"]').exists()).toBe(true);
  });

  it("warns when the server is unreachable and says where it looked", async () => {
    tauriApp();
    mockFetch({});
    const w = await bootApp();
    expect(w.get('[role="alert"]').text()).toContain("Server unreachable at http://server:8080");
  });

  it("hydrates the now-playing bar from the engine at launch", async () => {
    tauriApp(makeState({ status: "paused", track: makeTrack({ title: "Restored track" }) }));
    online();
    const w = await bootApp();
    expect(w.get('[data-testid="title"]').text()).toBe("Restored track");
  });

  it("shows a queue the engine restored from disk, with no further events (regression: empty Queue after restart)", async () => {
    const tracks = [makeTrack({ title: "One" }), makeTrack({ title: "Two" }), makeTrack({ title: "Three" })];
    tauriApp(makeState({ status: "stopped", track: tracks[1], queue_ids: tracks.map((t) => t.id), queue_index: 1 }));
    tauri.on("get_queue_tracks", tracks);
    online();
    await bootApp();
    const q = useQueueStore();
    expect(q.tracks.map((t) => t.title)).toEqual(["One", "Two", "Three"]);
    expect(q.index).toBe(1);
  });

  it("restores the queue from the saved copy even when the server is down", async () => {
    const tracks = [makeTrack({ title: "One" }), makeTrack({ title: "Two" })];
    tauriApp(makeState({ status: "stopped", track: tracks[0], queue_ids: tracks.map((t) => t.id), queue_index: 0 }));
    tauri.on("get_queue_tracks", tracks);
    mockFetch({}); // nothing answers: no server
    await bootApp();
    expect(useQueueStore().tracks.map((t) => t.title)).toEqual(["One", "Two"]);
  });

  it("reflects live engine events after launch (regression: UI went stale)", async () => {
    tauriApp();
    online();
    const w = await bootApp();
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ title: "Next up" }) }));
    await settle();
    expect(w.get('[data-testid="title"]').text()).toBe("Next up");
  });

  it("reloads the library when the server pushes a catalog-updated event (a scan finished — including one started elsewhere, like the desktop Server app)", async () => {
    tauriApp();
    const calls = online();
    await bootApp();
    const albumCallsAtBoot = calls.filter((c) => c.url.includes("/api/albums")).length;

    latestEventSource()!.emit("catalog-updated");
    await settle();

    const albumCallsAfter = calls.filter((c) => c.url.includes("/api/albums")).length;
    expect(albumCallsAfter).toBeGreaterThan(albumCallsAtBoot);
  });
});

describe("App: navigation", () => {
  it("navigates with the sidebar and shows the matching view", async () => {
    tauriApp();
    online();
    const w = await bootApp();
    for (const [label, heading] of [["Artists", "Artists"], ["Playlists", "Playlists"], ["Queue", "Queue"], ["Search", "Search"], ["Albums", "Albums"]] as const) {
      await w.findAll("nav button").find((b) => b.text().startsWith(label))!.trigger("click");
      await settle();
      expect(w.get("main h2").text()).toBe(heading);
    }
    await w.findAll("aside button").find((b) => b.text() === "Settings")!.trigger("click");
    await settle();
    expect(w.get("main h2").text()).toBe("Settings");
  });

  it("opens an album from the grid and returns", async () => {
    tauriApp();
    online();
    mockFetch({
      "/api/albums/": { album: { ...albums[0], track_ids: [] }, tracks: [] },
      "/api/health": { status: "ok" },
      "/api/albums": { items: albums, page: 1, per_page: 500, total: 1 },
      "/api/artists": [],
      "/api/playlists": [],
      "/api/jobs": [],
    });
    const w = await bootApp();
    await w.get("main .group").trigger("click");
    await settle();
    expect(useNavStore().view.name).toBe("album");
    await w.findAll("main button").find((b) => b.text().includes("Albums"))!.trigger("click");
    expect(useNavStore().view.name).toBe("albums");
  });
});

describe("App: global keyboard shortcuts", () => {
  async function ready() {
    tauriApp();
    online();
    const w = await bootApp();
    tauri.calls.length = 0;
    return w;
  }

  it.each([["1", "albums"], ["2", "artists"], ["3", "playlists"], ["4", "search"], ["5", "queue"], ["6", "settings"], ["f", "search"]])(
    "%s goes to %s",
    async (k, view) => {
      await ready();
      key(document.body, k);
      await settle();
      expect(useNavStore().view.name).toBe(view);
    },
  );

  it("A, B and X pick or swap the analog-warmth slot from any screen, and say what is playing", async () => {
    await ready();
    const analog = useAnalogStore();
    const toasts = useToastsStore();
    const sent = () => tauri.callsTo("set_analog").map((c) => (c as { settings: { enabled: boolean } }).settings.enabled);
    expect(analog.active).toBe("a");
    key(document.body, "b");
    await settle();
    expect(analog.active).toBe("b");
    expect(sent().at(-1)).toBe(true); // B is the warm one
    expect(toasts.toasts.at(-1)).toMatchObject({ title: "Analog warmth: listening to B" });
    expect(toasts.toasts.at(-1)?.detail).toContain("12AX7");
    key(document.body, "a");
    expect(analog.active).toBe("a");
    expect(sent().at(-1)).toBe(false);
    expect(toasts.toasts.at(-1)?.detail).toBe("Off (dry signal)");
    key(document.body, "X");
    expect(analog.active).toBe("b");
    key(document.body, "x");
    expect(analog.active).toBe("a");
    expect(toasts.toasts.filter((t) => t.title.startsWith("Analog warmth"))).toHaveLength(1); // one toast, replaced each time
  });

  it("A / B / X are ignored while typing", async () => {
    await ready();
    const input = document.createElement("input");
    document.body.appendChild(input);
    key(input, "b");
    expect(useAnalogStore().active).toBe("a");
    input.remove();
  });

  it("Space toggles playback", async () => {
    await ready();
    const e = key(document.body, " ");
    expect(e.defaultPrevented).toBe(true);
    expect(tauri.callsTo("toggle")).toHaveLength(1);
  });

  it("← / → seek by 10 s; ↑ / ↓ change volume by 5 %", async () => {
    await ready();
    key(document.body, "ArrowRight");
    expect((tauri.callsTo("seek_ms")[0] as { ms: number }).ms).toBeGreaterThanOrEqual(70_000);
    expect((tauri.callsTo("seek_ms")[0] as { ms: number }).ms).toBeLessThan(71_000);
    key(document.body, "ArrowLeft");
    key(document.body, "ArrowUp");
    key(document.body, "ArrowDown");
    await settle();
    const volumes = (tauri.callsTo("set_volume") as { v: number }[]).map((c) => c.v);
    expect(volumes[0]).toBeCloseTo(0.55, 5);
    expect(volumes).toHaveLength(2);
  });

  it("N and P skip tracks", async () => {
    await ready();
    key(document.body, "n");
    key(document.body, "p");
    expect(tauri.callsTo("next_track")).toHaveLength(1);
    expect(tauri.callsTo("prev_track")).toHaveLength(1);
  });

  it("does not hijack keys while typing in the search box", async () => {
    const w = await ready();
    useNavStore().go("search");
    await settle();
    const input = w.get("main input").element as HTMLInputElement;
    input.focus();
    await typeInto(input, "n");
    for (const k of ["n", "p", "1", " ", "ArrowRight"]) key(input, k);
    expect(tauri.callsTo("next_track")).toHaveLength(0);
    expect(tauri.callsTo("toggle")).toHaveLength(0);
    expect(tauri.callsTo("seek_ms")).toHaveLength(0);
    expect(useNavStore().view.name).toBe("search");
  });

  it("an arrow on the focused seek slider moves it ONCE (1 s), not also by the global 10 s", async () => {
    const w = await ready();
    const thumb = w.get('[role="slider"][aria-label="Seek"]').element;
    key(thumb, "ArrowRight");
    await settle();
    const seeks = tauri.callsTo("seek_ms") as { ms: number }[];
    expect(seeks).toHaveLength(1);
    expect(seeks[0].ms - 60_000).toBeGreaterThanOrEqual(1000);
    expect(seeks[0].ms - 60_000).toBeLessThan(2000);
  });

  it("an arrow on the focused volume slider adjusts it ONCE, not also by the global 5 %", async () => {
    const w = await ready();
    key(w.get('[role="slider"][aria-label="Volume"]').element, "ArrowUp");
    await settle();
    expect(tauri.callsTo("set_volume")).toEqual([{ v: 0.51 }]);
  });

  it("Space on a focused button presses it instead of toggling playback", async () => {
    const w = await ready();
    key(w.get("nav button").element, " ");
    expect(tauri.callsTo("toggle")).toHaveLength(0);
  });

  it("does nothing for Cmd/Ctrl combos (leaves them to the OS)", async () => {
    await ready();
    key(document.body, "n", { metaKey: true });
    key(document.body, "1", { ctrlKey: true });
    expect(tauri.callsTo("next_track")).toHaveLength(0);
  });

  it("stops listening when the app unmounts (no leaked window handler)", async () => {
    const w = await ready();
    w.unmount();
    key(document.body, "n");
    expect(tauri.callsTo("next_track")).toHaveLength(0);
    void usePlayerStore;
  });
});

describe("App: blind test keys", () => {
  it("during a blind test X means 'hear X', and the message never says which slot is playing", async () => {
    tauriApp();
    online();
    await bootApp();
    const analog = useAnalogStore();
    const abx = useAbxStore();
    const toasts = useToastsStore();
    analog.update("b", { flavour: "tube_300b" });
    analog.measured.b = 0.1;
    abx.start(3, () => 0.9);
    key(document.body, "x");
    expect(abx.heard).toBe("x");
    expect(toasts.toasts.at(-1)).toMatchObject({ title: "Blind test: hearing X", detail: null });
    key(document.body, "b");
    expect(abx.heard).toBe("b");
    expect(toasts.toasts.at(-1)?.title).toBe("Blind test: hearing B");
    expect(toasts.toasts.some((t) => t.title.startsWith("Analog warmth"))).toBe(false);
    expect(JSON.stringify(toasts.toasts)).not.toMatch(/300B|12AX7/);
  });
});

describe("App: About", () => {
  it("opens when the native menu's About item is chosen, and renders the page", async () => {
    tauriApp();
    online();
    await bootApp();
    expect(document.body.querySelector('[data-testid="about-dialog"]')).toBeNull();
    tauri.emit("menu-action", "some.other.item"); // ignored
    await settle();
    expect(useOverlaysStore().aboutOpen).toBe(false);
    tauri.emit("menu-action", "app.about");
    await settle();
    expect(useOverlaysStore().aboutOpen).toBe(true);
    expect(document.body.querySelector('[data-testid="about-content"] h1')!.textContent).toBe("Kahawai Player");
  });

  it("does not listen for menu events outside the Tauri shell", async () => {
    const before = tauri.listeners.get("menu-action")?.size ?? 0;
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = undefined;
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    mockFetch({});
    await bootApp();
    expect(tauri.listeners.get("menu-action")?.size ?? 0).toBe(before);
  });
});
