import { flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeState, makeTrack, mockFetch } from "../test/fixtures";
import { tauri } from "../test/tauri-mock";
import type { AudiobookDetail } from "../types";
import { useAudiobooksStore, POSITION_SAVE_MS } from "./audiobooks";
import { usePlayerStore } from "./player";
import { useQueueStore } from "./queue";

const T0 = new Date("2026-03-01T10:00:00Z").getTime();

const partTrack = (id: number, ms: number) => makeTrack({ id, duration_ms: ms, title: `Part ${id}`, album: "Book", artist: "Author", album_id: null });

function book(over: Partial<AudiobookDetail> = {}): AudiobookDetail {
  return {
    id: 7,
    root_id: 1,
    title: "Book",
    author: "Author",
    narrator: null,
    series: null,
    series_index: null,
    year: null,
    cover_hash: null,
    duration_ms: 600_000,
    added_at: 0,
    finished_at: null,
    position_ms: 0,
    last_played_at: null,
    progress: 0,
    parts: [
      { id: 1, track_id: 101, part_index: 0, title: "One", start_offset_ms: 0, duration_ms: 200_000 },
      { id: 2, track_id: 102, part_index: 1, title: "Two", start_offset_ms: 200_000, duration_ms: 400_000 },
    ],
    chapters: [
      { id: 1, part_id: 1, title: "A", start_offset_ms: 0, duration_ms: 200_000 },
      { id: 2, part_id: 2, title: "B", start_offset_ms: 200_000, duration_ms: 150_000 },
      { id: 3, part_id: 2, title: "C", start_offset_ms: 350_000, duration_ms: 250_000 },
    ],
    bookmarks: [],
    settings: { speed: 1.5, skip_back_s: 15, skip_forward_s: 30 },
    ...over,
  };
}

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

interface Env {
  fetchCalls: { url: string; init?: RequestInit }[];
  store: ReturnType<typeof useAudiobooksStore>;
  player: ReturnType<typeof usePlayerStore>;
  setBook: (b: AudiobookDetail) => void;
}

async function setup(initial = book()): Promise<Env> {
  let current = initial;
  const fetchCalls = mockFetch({
    "/api/tracks/101": () => json(partTrack(101, 200_000)),
    "/api/tracks/102": () => json(partTrack(102, 400_000)),
    "/api/audiobooks/7/position": (_u: string, init?: RequestInit) => json({ book_offset_ms: JSON.parse(String(init?.body)).book_offset_ms, updated_at: Date.now(), finished: false }),
    "/api/audiobooks/7/settings": (_u: string, init?: RequestInit) => json({ ...current.settings, ...JSON.parse(String(init?.body)) }),
    "/api/audiobooks/7/bookmarks": (_u: string, init?: RequestInit) =>
      json({ id: 55, book_id: 7, book_offset_ms: JSON.parse(String(init?.body)).book_offset_ms, name: "Bookmark", note: "", created_at: Date.now() }, 201),
    "/api/audiobooks/7/history": () => json([]),
    "/api/audiobooks/7": () => json(current),
    "/api/audiobooks": () => json([]),
  });
  tauri.on("get_state", makeState({ status: "stopped", track: null }));
  const player = usePlayerStore();
  await player.init();
  const store = useAudiobooksStore();
  return { fetchCalls, store, player, setBook: (b) => (current = b) };
}

/** What the engine reports while a part plays. */
function engine(trackId: number, positionMs: number, status: "playing" | "paused" | "stopped" = "playing", extra: Record<string, unknown> = {}) {
  const track = partTrack(trackId, trackId === 101 ? 200_000 : 400_000);
  tauri.emit("player-state", makeState({ status, track, position_ms: positionMs, duration_ms: track.duration_ms, playback_rate: 1.5, ...extra }));
}

const puts = (e: Env) => e.fetchCalls.filter((c) => c.url.endsWith("/api/audiobooks/7/position") && c.init?.method === "PUT").map((c) => JSON.parse(String(c.init?.body)).book_offset_ms as number);

beforeEach(() => {
  setActivePinia(createPinia());
  tauri.reset();
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval", "setTimeout", "clearTimeout"] });
  vi.setSystemTime(T0);
});
afterEach(() => vi.useRealTimers());

describe("starting a book", () => {
  it("puts the music aside, applies the book's speed, and opens the part at the saved offset", async () => {
    const e = await setup(book({ position_ms: 250_000 }));
    const queue = useQueueStore();
    const music = [makeTrack({ id: 1 }), makeTrack({ id: 2 })];
    queue.tracks = music;
    queue.index = 1;
    tauri.emit("player-state", makeState({ status: "playing", track: music[1], position_ms: 42_000, queue_ids: [1, 2], queue_index: 1, repeat: "all", shuffle: true }));

    await e.store.start(7);
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1.5 });
    const play = tauri.callsTo("queue_play_at").at(-1) as { index: number; position_ms: number; tracks: { id: number }[] };
    expect(play.tracks.map((t) => t.id)).toEqual([101, 102]);
    expect(play.index).toBe(1); // 250 s is in part two
    expect(play.position_ms).toBe(50_000);
    expect(e.store.stash).toMatchObject({ index: 1, positionMs: 42_000, repeat: "all", shuffle: true, wasPlaying: true });
    expect(e.store.stash?.tracks.map((t) => t.id)).toEqual([1, 2]);
  });

  it("a finished book starts again from the beginning", async () => {
    const e = await setup(book({ position_ms: 590_000, finished_at: 5 }));
    await e.store.start(7);
    const play = tauri.callsTo("queue_play_at").at(-1) as { index: number; position_ms: number };
    expect([play.index, play.position_ms]).toEqual([0, 0]);
  });

  it("starting a second book does not replace the music that was put aside", async () => {
    const e = await setup();
    const queue = useQueueStore();
    queue.tracks = [makeTrack({ id: 1 })];
    queue.index = 0;
    await e.store.start(7);
    const first = e.store.stash;
    engine(101, 1000);
    await flushPromises();
    await e.store.start(7, 10_000);
    expect(e.store.stash).toBe(first);
    expect(e.store.stash?.tracks.map((t) => t.id)).toEqual([1]);
  });
});

describe("book offsets and saving", () => {
  it("reports the book offset (part start + place in the part) when it pauses", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(102, 30_000, "playing");
    await flushPromises();
    engine(102, 30_000, "paused");
    await flushPromises();
    expect(puts(e).at(-1)).toBe(230_000);
    expect(e.store.offsetMs).toBe(230_000);
  });

  it("saves every 10 seconds while playing, and not while paused", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(101, 5_000, "playing");
    await flushPromises();
    const before = puts(e).length;
    vi.setSystemTime(T0 + POSITION_SAVE_MS);
    await vi.advanceTimersByTimeAsync(POSITION_SAVE_MS);
    expect(puts(e).length).toBe(before + 1);
    engine(101, 25_000, "paused");
    await flushPromises();
    const paused = puts(e).length;
    await vi.advanceTimersByTimeAsync(POSITION_SAVE_MS * 3);
    expect(puts(e).length).toBe(paused);
  });

  it("saves when the book moves on to its next part", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(101, 199_000, "playing");
    await flushPromises();
    engine(102, 500, "playing");
    await flushPromises();
    expect(puts(e).at(-1)).toBe(200_500);
  });
});

describe("moving around", () => {
  it("a seek inside the part is a plain seek; across parts reopens at the place", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(101, 10_000);
    await flushPromises();
    await e.store.seekToOffset(120_000);
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 120_000 });
    await e.store.seekToOffset(300_000);
    const play = tauri.callsTo("queue_play_at").at(-1) as { index: number; position_ms: number };
    expect([play.index, play.position_ms]).toEqual([1, 100_000]);
    expect(puts(e).at(-1)).toBe(300_000);
  });

  it("skip uses the book's own seconds and stays inside the book", async () => {
    const e = await setup(book({ settings: { speed: 1, skip_back_s: 20, skip_forward_s: 45 } }));
    await e.store.start(7);
    engine(101, 10_000);
    await flushPromises();
    await e.store.skip(1);
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 55_000 });
    await e.store.skip(-1);
    expect(tauri.callsTo("seek_ms").at(-1)?.ms).toBeLessThanOrEqual(35_000);
    vi.setSystemTime(T0 + 6_000); // the player store's guard against stale events has lapsed
    engine(101, 5_000);
    await flushPromises();
    await e.store.skip(-1);
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 0 });
  });

  it("chapters: next goes to the next start, previous restarts then steps back", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(102, 100_000); // book offset 300 s: chapter B
    await flushPromises();
    expect(e.store.chapter?.title).toBe("B");
    await e.store.nextChapter();
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 150_000 }); // C starts at 350 s = 150 s into part two
    engine(102, 151_000); // 1 s into C
    await flushPromises();
    await e.store.previousChapter();
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 0 }); // within 3 s of C's start: back to B (200 s = part two's 0)
  });
});

describe("speed, bookmarks", () => {
  it("setting the speed changes the engine and is remembered per book", async () => {
    const e = await setup();
    await e.store.start(7);
    await e.store.setSpeed(1.75);
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1.75 });
    const save = e.fetchCalls.filter((c) => c.url.endsWith("/settings")).at(-1);
    expect(JSON.parse(String(save?.init?.body))).toEqual({ speed: 1.75 });
    expect(e.store.speed).toBe(1.75);
  });

  it("a bookmark is made at the current book offset", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(102, 12_000);
    await flushPromises();
    await e.store.addBookmark();
    const post = e.fetchCalls.filter((c) => c.url.endsWith("/bookmarks") && c.init?.method === "POST").at(-1);
    expect(JSON.parse(String(post?.init?.body)).book_offset_ms).toBe(212_000);
    expect(e.store.active?.bookmarks).toHaveLength(1);
  });
});

describe("switching back to music", () => {
  it("saves the book, restores the music exactly, and returns the speed to normal", async () => {
    const e = await setup();
    const queue = useQueueStore();
    const music = [makeTrack({ id: 1 }), makeTrack({ id: 2 })];
    queue.tracks = music;
    queue.index = 1;
    tauri.emit("player-state", makeState({ status: "playing", track: music[1], position_ms: 42_000, queue_ids: [1, 2], queue_index: 1 }));
    await e.store.start(7);
    engine(102, 20_000, "playing");
    await flushPromises();
    await e.store.returnToMusic();
    expect(tauri.callsTo("pause")).toHaveLength(1);
    expect(puts(e).at(-1)).toBe(220_000);
    const restore = tauri.callsTo("queue_restore").at(-1) as { index: number; position_ms: number; tracks: { id: number }[] };
    expect([restore.index, restore.position_ms, restore.tracks.map((t) => t.id)]).toEqual([1, 42_000, [1, 2]]);
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1 });
    expect(tauri.callsTo("resume")).toHaveLength(1);
    expect(e.store.active).toBeNull();
    expect(e.store.stash).toBeNull();
  });

  it("music that was paused stays paused when it comes back", async () => {
    const e = await setup();
    const queue = useQueueStore();
    queue.tracks = [makeTrack({ id: 1 })];
    queue.index = 0;
    tauri.emit("player-state", makeState({ status: "paused", track: queue.tracks[0], position_ms: 9_000 }));
    await e.store.start(7);
    engine(101, 1000);
    await flushPromises();
    await e.store.returnToMusic();
    expect(tauri.callsTo("queue_restore")).toHaveLength(1);
    expect(tauri.callsTo("resume")).toHaveLength(0);
  });

  it("playing other music while a book is loaded ends the book's context and resets the speed", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(101, 1000);
    await flushPromises();
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ id: 999 }) }));
    await flushPromises();
    expect(e.store.active).toBeNull();
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1 });
  });
});

describe("sleep timer", () => {
  it("fades the volume over the last 10 s, then pauses, saves, and puts the volume back", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(101, 1000, "playing", { volume: 0.8 });
    await flushPromises();
    e.store.startSleepMinutes(5);
    await vi.advanceTimersByTimeAsync(5 * 60_000 - 5_000); // 5 s left: half of the fade
    const mid = (tauri.callsTo("set_volume").at(-1) as { v: number }).v;
    expect(mid).toBeGreaterThan(0.2);
    expect(mid).toBeLessThan(0.6);
    await vi.advanceTimersByTimeAsync(6_000);
    await flushPromises();
    expect(tauri.callsTo("pause").length).toBeGreaterThanOrEqual(1);
    expect((tauri.callsTo("set_volume").at(-1) as { v: number }).v).toBeCloseTo(0.8);
    expect(e.store.sleep).toBeNull();
  });

  it("cancelling during the fade puts the volume back", async () => {
    const e = await setup();
    await e.store.start(7);
    engine(101, 1000, "playing", { volume: 0.6 });
    await flushPromises();
    e.store.startSleepMinutes(5);
    await vi.advanceTimersByTimeAsync(5 * 60_000 - 3_000);
    e.store.cancelSleep(true);
    expect((tauri.callsTo("set_volume").at(-1) as { v: number }).v).toBeCloseTo(0.6);
    expect(tauri.callsTo("pause")).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(tauri.callsTo("pause")).toHaveLength(0);
  });

  it("end of chapter counts down in wall-clock time at the speed and fires at the chapter's end", async () => {
    const e = await setup(book({ settings: { speed: 2, skip_back_s: 15, skip_forward_s: 30 } }));
    await e.store.start(7);
    engine(102, 130_000, "playing", { playback_rate: 2 }); // offset 330 s, chapter B ends at 350 s
    await flushPromises();
    e.store.startSleepEndOfChapter();
    expect(e.store.sleep).toEqual({ kind: "chapter", endOffsetMs: 350_000 });
    expect(e.store.sleepRemainingMs).toBe(10_000); // 20 s of book at 2x
    engine(102, 150_000, "playing", { playback_rate: 2 }); // reached the end
    await vi.advanceTimersByTimeAsync(500);
    await flushPromises();
    expect(tauri.callsTo("pause").length).toBeGreaterThanOrEqual(1);
    expect(e.store.sleep).toBeNull();
  });
});
