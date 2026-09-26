import { describe, expect, it } from "vitest";
import { makeAlbum, makeState, makeTrack } from "../test/fixtures";
import { mountApp, options, openSelect, pick, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import NowPlayingBar from "./NowPlayingBar.vue";

async function boot(state = makeState({ status: "playing", position_ms: 83_000 })) {
  tauri.on("get_state", state);
  const { wrapper } = mountApp(NowPlayingBar);
  await usePlayerStore().init();
  await settle();
  return wrapper;
}

describe("NowPlayingBar", () => {
  it("shows the current track's title and artist", async () => {
    const track = makeTrack({ title: "Smoke Signals", artist: "Phoebe Bridgers" });
    const w = await boot(makeState({ status: "playing", track }));
    expect(w.get('[data-testid="title"]').text()).toBe("Smoke Signals");
    expect(w.get('[data-testid="artist"]').text()).toBe("Phoebe Bridgers");
  });

  it("updates live when a player-state event changes the track (regression: stale title/artwork)", async () => {
    const w = await boot(makeState({ status: "playing", track: makeTrack({ title: "First" }) }));
    expect(w.get('[data-testid="title"]').text()).toBe("First");
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ title: "Second", artist: "B" }) }));
    await settle();
    expect(w.get('[data-testid="title"]').text()).toBe("Second");
    expect(w.get('[data-testid="artist"]').text()).toBe("B");
  });

  it("shows the album cover for the playing track through the artwork protocol", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const album = makeAlbum({ id: 42, artwork_hash: "cafe01" });
    const track = makeTrack({ album_id: 42 });
    const { wrapper } = mountApp(NowPlayingBar);
    useLibraryStore().albums = [album];
    tauri.on("get_state", makeState({ status: "playing", track }));
    await usePlayerStore().init();
    await settle();
    expect(wrapper.get("img").attributes("src")).toBe("artwork://localhost/cafe01");
    // ...and follows the track to a different album.
    const other = makeAlbum({ id: 43, artwork_hash: "beef02" });
    useLibraryStore().albums = [album, other];
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ album_id: 43 }) }));
    await settle();
    expect(wrapper.get("img").attributes("src")).toBe("artwork://localhost/beef02");
  });

  it("says nothing is playing and disables the controls when idle", async () => {
    const w = await boot(makeState({ track: null, queue_ids: [], queue_index: null }));
    expect(w.get('[data-testid="title"]').text()).toBe("Nothing playing");
    expect(w.get('[data-testid="artist"]').text()).toBe("—");
    const controls = w.findAll("button").filter((b) => ["Shuffle", "Previous", "Next", "Stop"].includes(b.attributes("aria-label") ?? ""));
    expect(controls.length).toBe(4);
    expect(controls.every((b) => b.attributes("disabled") !== undefined)).toBe(true);
    expect(w.find('[role="slider"][aria-label="Seek"]').attributes("data-disabled")).toBeDefined();
  });

  it("shows the audio chain and the Exclusive DoP badge", async () => {
    const w = await boot(makeState({ status: "playing", chain: "dsf64->dop", output_path: "dop-exclusive" }));
    expect(w.text()).toContain("dsf64->dop");
    expect(w.text()).toContain("Exclusive DoP");
    const pcm = await boot(makeState({ status: "playing", chain: "m4a->passthrough", output_path: "pcm-shared" }));
    expect(pcm.text()).not.toContain("Exclusive DoP");
  });

  it("shows a Bit-perfect badge only on the exclusive PCM path", async () => {
    const bp = await boot(makeState({ status: "playing", output_path: "pcm-exclusive" }));
    expect(bp.get('[data-testid="bit-perfect-badge"]').text()).toBe("Bit-perfect");
    expect(bp.text()).not.toContain("Exclusive DoP");
    for (const path of ["pcm-shared", "dop-exclusive"] as const) {
      const w = await boot(makeState({ status: "playing", output_path: path }));
      expect(w.find('[data-testid="bit-perfect-badge"]').exists()).toBe(false);
    }
  });

  it("surfaces an engine error as an alert", async () => {
    const w = await boot(makeState({ status: "stopped", error: "decode failed: boom" }));
    expect(w.get('[role="alert"]').text()).toContain("decode failed: boom");
    expect((await boot(makeState())).find('[role="alert"]').exists()).toBe(false);
  });

  it("opens the full now-playing view from the identity area, only when a track exists", async () => {
    const w = await boot();
    await w.get('[data-testid="identity"]').trigger("click");
    expect(useNavStore().view.name).toBe("nowplaying");

    const idle = await boot(makeState({ track: null, queue_ids: [], queue_index: null }));
    useNavStore().go("albums");
    await idle.get('[data-testid="identity"]').trigger("click");
    expect(useNavStore().view.name).toBe("albums");
  });

  it("goes to the queue from the queue button", async () => {
    const w = await boot();
    await w.get('button[aria-label="Queue"]').trigger("click");
    expect(useNavStore().view.name).toBe("queue");
  });

  it("seeks from the keyboard on its slider and shows the new time", async () => {
    const w = await boot(makeState({ status: "paused", position_ms: 83_000, track: makeTrack({ duration_ms: 318_742 }) }));
    await w.get('[role="slider"][aria-label="Seek"]').trigger("keydown", { key: "ArrowRight" });
    await settle();
    expect(tauri.callsTo("seek_ms")).toEqual([{ ms: 84_000 }]);
  });

  it("changes volume from its slider", async () => {
    const w = await boot(makeState({ status: "playing", volume: 0.5 }));
    await w.get('[role="slider"][aria-label="Volume"]').trigger("keydown", { key: "ArrowRight" });
    await settle();
    expect(tauri.callsTo("set_volume")).toEqual([{ v: 0.51 }]);
  });

  it("shows Auto until this track is overridden, even though it plays as PASSTHROUGH", async () => {
    const w = await boot(makeState({ status: "playing", track: makeTrack({ format: "flac" }), format: "passthrough", chain: "flac->passthrough" }));
    const trigger = w.get('[aria-label="Stream format for this track"]');
    expect(trigger.text()).toContain("Auto");
    expect(trigger.text()).not.toContain("PASSTHROUGH");
    // The tooltip says what Auto means and what it is doing right now.
    expect(trigger.attributes("title")).toContain("default from Settings");
    expect(trigger.attributes("title")).toContain("playing as PASSTHROUGH");
  });

  it("lists valid per-track formats, forces the chosen one, and then shows it", async () => {
    const track = makeTrack({ format: "flac" });
    const w = await boot(makeState({ status: "playing", track, format: "passthrough" }));
    const trigger = w.get('[aria-label="Stream format for this track"]');
    await openSelect(trigger.element as HTMLElement);
    expect(options().map((o) => o.textContent?.trim())).toEqual(["Auto", "PASSTHROUGH", "FLAC", "OPUS", "MP3"]);
    pick(options()[2]); // FLAC
    await settle();
    expect(tauri.callsTo("set_track_format")).toEqual([{ track_id: track.id, fmt: "flac" }]);
    expect(trigger.text()).toContain("FLAC");
    expect(trigger.attributes("title")).toContain("forced to FLAC");
  });

  it("remembers an override per track, not globally", async () => {
    const t1 = makeTrack({ format: "flac" });
    const t2 = makeTrack({ format: "flac" });
    const w = await boot(makeState({ status: "playing", track: t1 }));
    await openSelect(w.get('[aria-label="Stream format for this track"]').element as HTMLElement);
    pick(options()[3]); // OPUS for t1
    await settle();
    tauri.emit("player-state", makeState({ status: "playing", track: t2 }));
    await settle();
    expect(w.get('[aria-label="Stream format for this track"]').text()).toContain("Auto");
    tauri.emit("player-state", makeState({ status: "playing", track: t1 }));
    await settle();
    expect(w.get('[aria-label="Stream format for this track"]').text()).toContain("OPUS");
  });

  it("offers Auto as a real choice (null format)", async () => {
    const track = makeTrack({ format: "flac" });
    const w = await boot(makeState({ status: "playing", track, format: "flac" }));
    const trigger = w.get('[aria-label="Stream format for this track"]');
    await openSelect(trigger.element as HTMLElement);
    pick(options()[2]); // force FLAC first
    await settle();
    await openSelect(trigger.element as HTMLElement);
    pick(options()[0]); // back to Auto
    await settle();
    expect(tauri.callsTo("set_track_format").at(-1)).toEqual({ track_id: track.id, fmt: null });
    expect(trigger.text()).toContain("Auto");
  });

  it("only offers FLAC/DoP for DSD tracks", async () => {
    const w = await boot(makeState({ status: "playing", track: makeTrack({ format: "dsf" }), format: "flac" }));
    await openSelect(w.get('[aria-label="Stream format for this track"]').element as HTMLElement);
    expect(options().map((o) => o.textContent?.trim())).toEqual(["Auto", "FLAC", "DOP"]);
  });

  it("disables the format picker for an undecodable track and says why", async () => {
    const track = makeTrack({ decodable: false, format: "sacd_iso" });
    const w = await boot(makeState({ status: "stopped", track }));
    const trigger = w.get('[aria-label="Stream format for this track"]');
    expect(trigger.attributes("disabled")).toBeDefined();
    expect(trigger.attributes("title")).toMatch(/extraction/i);
  });

  it("has no gear/settings popover in the bar: the default format lives in Settings", async () => {
    const w = await boot();
    expect(w.find('button[aria-label="Playback settings"]').exists()).toBe(false);
    expect(w.text()).not.toContain("Default format");
    expect(tauri.callsTo("set_format")).toHaveLength(0);
  });
});

describe("NowPlayingBar icons", () => {
  it("every icon-only control is a real SVG icon with an accessible name (no emoji)", async () => {
    const w = await boot(makeState({ status: "playing" }));
    const iconOnly = w.findAll("button").filter((b) => b.find("svg").exists() && b.text().trim() === "");
    expect(iconOnly.length).toBeGreaterThanOrEqual(8); // 6 transport + queue + settings (+ menu)
    for (const b of iconOnly) {
      expect(b.attributes("aria-label") ?? b.attributes("title")).toBeTruthy();
    }
    expect(w.text()).not.toMatch(/[←-⇿⏩-⏺■-➿\u{1F300}-\u{1FAFF}]/u);
  });

  it("shows the alert icon next to an engine error", async () => {
    const w = await boot(makeState({ status: "stopped", error: "decode failed" }));
    expect(w.get('[role="alert"] svg').classes().join(" ")).toContain("lucide-triangle-alert");
  });
});
