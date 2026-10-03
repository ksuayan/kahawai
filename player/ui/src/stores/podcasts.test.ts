import { flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { episodeTrack, episodeTrackId } from "../lib/podcast";
import { makeEpisode, makeState, makeTrack, mockFetch } from "../test/fixtures";
import { tauri } from "../test/tauri-mock";
import type { PodcastEpisode, PodcastFeed } from "../types";
import { useAudiobooksStore } from "./audiobooks";
import { usePlayerStore } from "./player";
import { POSITION_SAVE_MS, usePodcastsStore } from "./podcasts";
import { useQueueStore } from "./queue";

const T0 = new Date("2026-03-01T10:00:00Z").getTime();
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

function feed(over: Partial<PodcastFeed> = {}): PodcastFeed {
  return {
    id: 3,
    feed_url: "https://show.example/feed",
    title: "The Show",
    author: "Host",
    description: null,
    link: null,
    image_url: null,
    language: null,
    explicit: false,
    last_fetched: null,
    last_error: null,
    auto_download: true,
    keep_n: 5,
    delete_played_after_days: 7,
    sort_order: 0,
    added_at: 0,
    episode_count: 3,
    unplayed_count: 3,
    speed: 1.5,
    skip_back_s: 10,
    skip_forward_s: 45,
    auto_advance: false,
    ...over,
  };
}

const ep1 = makeEpisode({ id: 11, title: "One", position_ms: 120_000, published_at: 1000 });
const ep2 = makeEpisode({ id: 12, title: "Two", published_at: 2000 });
const ep3 = makeEpisode({ id: 13, title: "Three", published_at: 3000 });

async function setup(f = feed(), unplayed: PodcastEpisode[] = [ep3, ep2, ep1]) {
  const calls = mockFetch({
    "/api/podcasts/feeds/3/episodes": () => json(unplayed),
    "/api/podcasts/feeds/3/settings": (_u: string, init?: RequestInit) => json({ ...f, ...JSON.parse(String(init?.body)) }),
    "/api/podcasts/feeds": () => json([f]),
    "/api/podcasts/in-progress": () => json([]),
    "/api/podcasts/episodes/11/position": (_u: string, init?: RequestInit) => json({ offset_ms: JSON.parse(String(init?.body)).offset_ms, updated_at: Date.now(), played: false }),
    "/api/podcasts/episodes/12/position": () => json({ offset_ms: 0, updated_at: 1, played: false }),
    "/api/podcasts/episodes/11": () => json({ ...ep1, feed: f }),
    "/api/podcasts/episodes/12": () => json({ ...ep2, feed: f }),
    "/api/podcasts/episodes/13": () => json({ ...ep3, feed: f }),
  });
  tauri.on("get_state", makeState({ status: "stopped", track: null }));
  const player = usePlayerStore();
  await player.init();
  const store = usePodcastsStore();
  await store.loadFeeds();
  return { calls, store, player };
}

/** What the engine reports while an episode plays. */
function engine(epId: number, positionMs: number, status: "playing" | "paused" | "stopped" = "playing") {
  const track = { ...makeTrack({ id: episodeTrackId(epId), duration_ms: 600_000 }), title: `Episode ${epId}` };
  tauri.emit("player-state", makeState({ status, track, position_ms: positionMs, duration_ms: 600_000, playback_rate: 1.5 }));
}

const positionPuts = (calls: { url: string; init?: RequestInit }[], id = 11) =>
  calls.filter((c) => c.url.endsWith(`/api/podcasts/episodes/${id}/position`)).map((c) => JSON.parse(String(c.init?.body)).offset_ms as number);

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
  tauri.reset();
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval", "setTimeout", "clearTimeout"] });
  vi.setSystemTime(T0);
});
afterEach(() => vi.useRealTimers());

describe("playing an episode", () => {
  it("puts the music aside, applies the show's speed, and starts where you left off with Up Next behind it", async () => {
    const e = await setup();
    const queue = useQueueStore();
    const music = [makeTrack({ id: 1 }), makeTrack({ id: 2 })];
    queue.tracks = music;
    queue.index = 1;
    tauri.emit("player-state", makeState({ status: "playing", track: music[1], position_ms: 42_000, queue_ids: [1, 2], queue_index: 1 }));
    e.store.upNext = [ep2];

    await e.store.play(ep1);
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1.5 });
    const play = tauri.callsTo("queue_play_at").at(-1) as { index: number; positionMs: number; tracks: { id: number }[] };
    expect(play.tracks.map((t) => t.id)).toEqual([episodeTrackId(11), episodeTrackId(12)]);
    expect([play.index, play.positionMs]).toEqual([0, 120_000]);
    expect(e.store.stash?.tracks.map((t) => t.id)).toEqual([1, 2]);
    expect(e.store.stash).toMatchObject({ index: 1, positionMs: 42_000, wasPlaying: true });
    expect(e.store.active?.episode.id).toBe(11);
  });

  it("saves the place every 10 s while playing and when it pauses", async () => {
    const e = await setup();
    await e.store.play(ep1);
    engine(11, 125_000);
    await flushPromises();
    vi.advanceTimersByTime(POSITION_SAVE_MS);
    await flushPromises();
    expect(positionPuts(e.calls)).toContain(125_000);
    engine(11, 130_000, "paused");
    await flushPromises();
    expect(positionPuts(e.calls).at(-1)).toBe(130_000);
  });

  it("skips by the show's own seconds", async () => {
    const e = await setup();
    await e.store.play(ep1);
    engine(11, 100_000);
    await flushPromises();
    await e.store.skip(1);
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 145_000 });
    await e.store.skip(-1);
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 135_000 }); // 10 s back from 145 s
  });

  it("Back to music saves the place and brings the music back exactly", async () => {
    const e = await setup();
    const queue = useQueueStore();
    queue.tracks = [makeTrack({ id: 1 })];
    queue.index = 0;
    tauri.emit("player-state", makeState({ status: "playing", track: queue.tracks[0], position_ms: 7_000, queue_ids: [1], queue_index: 0 }));
    await e.store.play(ep1);
    engine(11, 200_000);
    await flushPromises();
    await e.store.returnToMusic();
    expect(positionPuts(e.calls).at(-1)).toBe(200_000);
    const restore = tauri.callsTo("queue_restore").at(-1) as { tracks: { id: number }[]; positionMs: number };
    expect(restore.tracks.map((t) => t.id)).toEqual([1]);
    expect(restore.positionMs).toBe(7_000);
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1 });
    expect(e.store.active).toBeNull();
  });

  it("other music taking over ends the podcast and returns the speed to normal", async () => {
    const e = await setup();
    await e.store.play(ep1);
    engine(11, 1_000);
    await flushPromises();
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ id: 5 }) }));
    await flushPromises();
    expect(e.store.active).toBeNull();
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1 });
  });

  it("a book taking over keeps the music put aside and lets the book set the speed", async () => {
    const e = await setup();
    const queue = useQueueStore();
    queue.tracks = [makeTrack({ id: 1 })];
    queue.index = 0;
    tauri.emit("player-state", makeState({ status: "playing", track: queue.tracks[0], queue_ids: [1], queue_index: 0 }));
    await e.store.play(ep1);
    engine(11, 1_000);
    await flushPromises();
    const books = useAudiobooksStore();
    books.active = { id: 7, parts: [{ id: 1, track_id: 101, part_index: 0, title: null, start_offset_ms: 0, duration_ms: 1000 }] } as never;
    const ratesBefore = tauri.callsTo("set_playback_rate").length;
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ id: 101 }) }));
    await flushPromises();
    expect(e.store.active).toBeNull();
    expect(e.store.stash?.tracks.map((t) => t.id)).toEqual([1]);
    expect(tauri.callsTo("set_playback_rate")).toHaveLength(ratesBefore);
  });

  it("when the engine moves on to the next episode, that one is playing: Up Next shrinks and the place is used", async () => {
    const e = await setup();
    const saved = { ...ep2, position_ms: 30_000 };
    mockFetch({
      "/api/podcasts/episodes/12/position": () => json({ offset_ms: 0, updated_at: 1, played: false }),
      "/api/podcasts/episodes/11/position": () => json({ offset_ms: 600_000, updated_at: 1, played: true }),
      "/api/podcasts/episodes/12": () => json({ ...saved, feed: feed() }),
      "/api/podcasts/feeds": () => json([feed()]),
    });
    e.store.upNext = [saved];
    await e.store.play(ep1);
    engine(11, 590_000);
    await flushPromises();
    engine(12, 0);
    await flushPromises();
    await flushPromises();
    expect(e.store.active?.episode.id).toBe(12);
    expect(e.store.upNext).toEqual([]);
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 30_000 });
  });

  it("is taken up again after a restart when the engine still holds an episode", async () => {
    const e = await setup();
    engine(11, 50_000, "paused");
    await flushPromises();
    await e.store.adopt();
    expect(e.store.active?.episode.id).toBe(11);
    expect(e.store.active?.feed.title).toBe("The Show");
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1.5 });
  });
});

describe("Up Next", () => {
  it("is kept across launches", async () => {
    const e = await setup();
    await e.store.addToUpNext(ep2);
    await flushPromises();
    setActivePinia(createPinia());
    expect(usePodcastsStore().upNext.map((x) => x.id)).toEqual([12]);
    expect(e.store.inUpNext(12)).toBe(true);
  });

  it("while a podcast plays, an added episode joins the engine's queue, and a removed one leaves it", async () => {
    const e = await setup();
    await e.store.play(ep1);
    engine(11, 1_000);
    await flushPromises();
    await e.store.addToUpNext(ep2);
    const appended = tauri.callsTo("queue_append").at(-1) as { tracks: { id: number }[] };
    expect(appended.tracks.map((t) => t.id)).toEqual([episodeTrackId(12)]);
    const queue = useQueueStore();
    queue.tracks = [episodeTrack(ep1), episodeTrack(ep2)];
    queue.index = 0;
    await e.store.removeFromUpNext(12);
    expect(tauri.callsTo("queue_remove").at(-1)).toEqual({ index: 1 });
    expect(e.store.upNext).toEqual([]);
  });

  it("with auto-advance and nothing in Up Next, the show's next unplayed episode is queued", async () => {
    const e = await setup(feed({ auto_advance: true }));
    await e.store.play(ep1);
    await flushPromises();
    const appended = tauri.callsTo("queue_append").at(-1) as { tracks: { id: number }[] };
    expect(appended.tracks.map((t) => t.id)).toEqual([episodeTrackId(12)]); // the one after it, not the newest
  });
});

describe("shows", () => {
  it("saving a playing show's speed changes the engine too", async () => {
    const e = await setup();
    await e.store.play(ep1);
    engine(11, 1_000);
    await flushPromises();
    await e.store.setSpeed(2);
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 2 });
    const put = e.calls.filter((c) => c.url.endsWith("/api/podcasts/feeds/3/settings")).at(-1);
    expect(JSON.parse(String(put?.init?.body))).toEqual({ speed: 2 });
    expect(e.store.speed).toBe(2);
  });

  it("a subscription the server refuses says why", async () => {
    const e = await setup();
    mockFetch({ "/api/podcasts/feeds": () => json({ error: "that address is a web page, not a feed" }, 400) });
    expect(await e.store.subscribe("https://example.com")).toBeNull();
    const { useToastsStore } = await import("./toasts");
    expect(useToastsStore().toasts.at(-1)?.detail).toContain("web page");
  });
});
