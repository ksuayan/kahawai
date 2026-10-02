import { flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeState } from "../test/fixtures";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "./player";

const T0 = new Date("2026-01-01T00:00:00Z").getTime();

async function ready(initial = makeState({ status: "playing", position_ms: 10_000 })) {
  tauri.on("get_state", initial);
  const p = usePlayerStore();
  await p.init();
  return p;
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval", "setTimeout", "clearTimeout"] });
  vi.setSystemTime(T0);
});
afterEach(() => vi.useRealTimers());

describe("hydration and events", () => {
  it("hydrates from get_state on launch (queue restored before the listener attached)", async () => {
    const p = await ready(makeState({ status: "paused", position_ms: 42_000 }));
    expect(p.connected).toBe(true);
    expect(p.status).toBe("paused");
    expect(p.positionMs).toBe(42_000);
  });

  it("applies live player-state events (title, artwork track, status)", async () => {
    const p = await ready(makeState({ status: "stopped" }));
    const next = makeState({ status: "playing", position_ms: 5_000 });
    tauri.emit("player-state", next);
    expect(p.currentTrack?.id).toBe(next.track!.id);
    expect(p.isPlaying).toBe(true);
  });

  it("exposes derived fields from the snapshot", async () => {
    const p = await ready(
      makeState({ status: "playing", volume: 0.4, repeat: "all", shuffle: true, output_path: "dop-exclusive", chain: "a->b" }),
    );
    expect(p.volume).toBe(0.4);
    expect(p.repeat).toBe("all");
    expect(p.shuffle).toBe(true);
    expect(p.isDopExclusive).toBe(true);
    expect(p.chain).toBe("a->b");
    expect(p.durationMs).toBe(200_000);
  });
});

describe("position", () => {
  it("interpolates between events while playing", async () => {
    const p = await ready(makeState({ status: "playing", position_ms: 10_000 }));
    expect(p.positionMs).toBe(10_000);
    vi.setSystemTime(T0 + 1000);
    vi.advanceTimersByTime(500); // advances the clock too; the tick recomputes the display
    expect(p.positionMs).toBe(11_500);
  });

  it("interpolates at the playback speed (the playhead is media time)", async () => {
    const p = await ready(makeState({ status: "playing", position_ms: 10_000, playback_rate: 2 }));
    expect(p.playbackRate).toBe(2);
    vi.setSystemTime(T0 + 1000);
    vi.advanceTimersByTime(500);
    expect(p.positionMs).toBe(13_000); // 1.5 s of wall clock at 2x
  });

  it("does not advance while paused or stopped", async () => {
    const p = await ready(makeState({ status: "paused", position_ms: 10_000 }));
    vi.setSystemTime(T0 + 5000);
    vi.advanceTimersByTime(1000);
    expect(p.positionMs).toBe(10_000);
  });

  it("clamps at the track duration and never goes negative", async () => {
    const p = await ready(makeState({ status: "playing", position_ms: 199_500 }));
    vi.setSystemTime(T0 + 60_000);
    vi.advanceTimersByTime(500);
    expect(p.positionMs).toBe(200_000);
    tauri.emit("player-state", makeState({ status: "paused", position_ms: -50 }));
    expect(p.positionMs).toBe(0);
  });

  it("resets the interpolation base on every event", async () => {
    const p = await ready(makeState({ status: "playing", position_ms: 0 }));
    vi.setSystemTime(T0 + 4000);
    tauri.emit("player-state", makeState({ status: "playing", position_ms: 3000 }));
    expect(p.positionMs).toBe(3000);
  });
});

describe("seeking", () => {
  it("moves the display immediately (optimistic) and calls seek_ms once", async () => {
    const p = await ready(makeState({ status: "playing", position_ms: 10_000 }));
    await p.seekTo(90_000.4);
    expect(p.positionMs).toBe(90_000);
    expect(tauri.callsTo("seek_ms")).toEqual([{ ms: 90_000 }]);
  });

  it("ignores in-flight events that still carry the OLD playhead (no snap-back)", async () => {
    const p = await ready(makeState({ status: "playing", position_ms: 10_000 }));
    await p.seekTo(120_000);
    // An event emitted before the engine applied the seek:
    tauri.emit("player-state", makeState({ status: "playing", position_ms: 10_400 }));
    expect(p.positionMs).toBe(120_000);
    // The engine catches up: the guard releases and real positions flow again.
    tauri.emit("player-state", makeState({ status: "playing", position_ms: 120_300 }));
    expect(p.positionMs).toBe(120_300);
    tauri.emit("player-state", makeState({ status: "playing", position_ms: 121_000 }));
    expect(p.positionMs).toBe(121_000);
  });

  it("stops guarding after a few seconds so a failed seek shows the truth", async () => {
    const p = await ready(makeState({ status: "playing", position_ms: 10_000 }));
    await p.seekTo(120_000);
    vi.setSystemTime(T0 + 6000);
    tauri.emit("player-state", makeState({ status: "playing", position_ms: 16_000 }));
    expect(p.positionMs).toBe(16_000);
  });

  it("clamps negative seeks to zero at the bridge", async () => {
    const p = await ready();
    await p.seekTo(-500);
    expect(tauri.callsTo("seek_ms")).toEqual([{ ms: 0 }]);
  });
});

describe("volume", () => {
  it("updates optimistically and clamps at the bridge", async () => {
    const p = await ready(makeState({ volume: 1 }));
    await p.changeVolume(0.25);
    expect(p.volume).toBe(0.25);
    await p.changeVolume(3);
    await p.changeVolume(-1);
    expect(tauri.callsTo("set_volume")).toEqual([{ v: 0.25 }, { v: 1 }, { v: 0 }]);
  });

  it("is overwritten by the engine's confirmed value on the next event", async () => {
    const p = await ready(makeState({ volume: 1 }));
    await p.changeVolume(0.5);
    tauri.emit("player-state", makeState({ volume: 0.5 }));
    expect(p.volume).toBe(0.5);
  });
});

describe("when the event subscription is refused (missing core:event capability)", () => {
  it("falls back to polling get_state so the UI does not go stale", async () => {
    tauri.listenFails = true;
    const first = makeState({ status: "playing", position_ms: 1000 });
    tauri.on("get_state", first);
    const p = usePlayerStore();
    await p.init();
    expect(p.positionMs).toBe(1000);

    const changed = makeState({ status: "playing", position_ms: 8000 });
    tauri.on("get_state", changed);
    vi.advanceTimersByTime(350);
    await flushPromises();
    expect(p.currentTrack?.id).toBe(changed.track!.id);
    expect(p.positionMs).toBeGreaterThanOrEqual(8000);
  });

  it("does not poll outside Tauri (get_state returns nothing)", async () => {
    tauri.listenFails = true;
    const p = usePlayerStore();
    await p.init();
    const before = tauri.callsTo("get_state").length;
    vi.advanceTimersByTime(2000);
    await flushPromises();
    expect(tauri.callsTo("get_state").length).toBe(before);
    p.dispose();
  });
});

describe("transport commands", () => {
  it("forwards toggle/next/prev to the bridge", async () => {
    const p = await ready();
    await p.toggle();
    await p.nextTrack();
    await p.prevTrack();
    expect(tauri.calls.map((c) => c.cmd)).toEqual(expect.arrayContaining(["toggle", "next_track", "prev_track"]));
  });
});

describe("per-track format overrides", () => {
  it("has no override until one is chosen", async () => {
    const p = await ready();
    expect(p.formatOverride(1)).toBeNull();
    expect(p.formatOverride(null)).toBeNull();
    expect(p.formatOverride(undefined)).toBeNull();
  });

  it("remembers the forced format per track and sends it to the engine", async () => {
    const p = await ready();
    await p.changeTrackFormat(7, "flac");
    await p.changeTrackFormat(8, "opus");
    expect(p.formatOverride(7)).toBe("flac");
    expect(p.formatOverride(8)).toBe("opus");
    expect(p.formatOverride(9)).toBeNull();
    expect(tauri.callsTo("set_track_format")).toEqual([{ track_id: 7, fmt: "flac" }, { track_id: 8, fmt: "opus" }]);
  });

  it("Auto (null) clears the override and tells the engine", async () => {
    const p = await ready();
    await p.changeTrackFormat(7, "flac");
    await p.changeTrackFormat(7, null);
    expect(p.formatOverride(7)).toBeNull();
    expect(tauri.callsTo("set_track_format").at(-1)).toEqual({ track_id: 7, fmt: null });
  });

  it("changing the format of one track leaves the others alone", async () => {
    const p = await ready();
    await p.changeTrackFormat(1, "mp3");
    await p.changeTrackFormat(2, "flac");
    await p.changeTrackFormat(1, null);
    expect(p.formatOverride(2)).toBe("flac");
  });
});

describe("output path", () => {
  it.each([
    ["pcm-shared", false, false, false],
    ["dop-exclusive", true, false, true],
    ["pcm-exclusive", false, true, true],
  ] as const)("%s -> dop=%s bitPerfect=%s exclusive=%s", async (path, dop, bp, excl) => {
    const p = await ready(makeState({ status: "playing", output_path: path }));
    expect(p.isDopExclusive).toBe(dop);
    expect(p.isBitPerfect).toBe(bp);
    expect(p.isExclusive).toBe(excl);
  });

  it("follows the engine when the path changes (bit-perfect switched on mid-track)", async () => {
    const p = await ready(makeState({ status: "playing", output_path: "pcm-shared" }));
    expect(p.isBitPerfect).toBe(false);
    tauri.emit("player-state", makeState({ status: "playing", output_path: "pcm-exclusive" }));
    expect(p.isBitPerfect).toBe(true);
    expect(p.isExclusive).toBe(true);
  });
});
