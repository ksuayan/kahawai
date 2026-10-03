import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import {
  deletePodcastDownload,
  downloadPodcastEpisode,
  fetchPodcastDownloads,
  fetchPodcastEpisode,
  fetchPodcastEpisodeHistory,
  fetchPodcastEpisodes,
  fetchPodcastFeeds,
  fetchPodcastFolder,
  fetchPodcastsInProgress,
  importPodcastOpml,
  markPodcastEpisodePlayed,
  refreshPodcasts,
  savePodcastFeedSettings,
  savePodcastPosition,
  subscribePodcast,
  unsubscribePodcast,
} from "../api";
import { clampSpeed } from "../lib/audiobook";
import { episodeOfTrack, episodeTrack, isEpisodeTrack, startOffset } from "../lib/podcast";
import { uiGet, uiSet } from "../lib/uiState";
import { queuePlayAt, queueRestore, setPlaybackRate } from "../tauri";
import type {
  AudiobookSession,
  JobInfo,
  PodcastEpisode,
  PodcastEpisodeDetail,
  PodcastFeed,
  PodcastFeedSettings,
  PodcastFolder,
} from "../types";
import { isJobActive } from "../types";
import { useAudiobooksStore } from "./audiobooks";
import { useJobsStore } from "./jobs";
import { useMusicStashStore } from "./musicStash";
import { usePlayerStore } from "./player";
import { useQueueStore } from "./queue";
import { useToastsStore } from "./toasts";

/** How often the position is saved while an episode plays (ms), like books. */
export const POSITION_SAVE_MS = 10_000;
const UP_NEXT_KEY = "kahawai.podcasts.upNext";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function loadUpNext(): PodcastEpisode[] {
  try {
    const v = JSON.parse(uiGet(UP_NEXT_KEY) ?? "[]") as unknown;
    return Array.isArray(v) ? (v.filter((x) => x && typeof x === "object" && typeof (x as PodcastEpisode).id === "number") as PodcastEpisode[]) : [];
  } catch {
    return [];
  }
}

/**
 * Podcasts: subscriptions, episodes, downloads, and the podcast listening
 * context. An episode plays through the engine like a track (the server
 * streams it, downloaded or not); the Up Next episodes ride behind it in the
 * engine's queue, so it moves on by itself. Starting an episode puts the music
 * aside (shared with audiobooks: stores/musicStash), and Back to music brings
 * it back exactly. The position is saved every 10 s and on pause, stop and
 * skip; the server marks an episode played at 97%.
 */
export const usePodcastsStore = defineStore("podcasts", () => {
  const player = usePlayerStore();
  const queue = useQueueStore();
  const toasts = useToastsStore();
  const stashStore = useMusicStashStore();

  // --- subscriptions -------------------------------------------------------------
  const feeds = ref<PodcastFeed[]>([]);
  const inProgress = ref<PodcastEpisode[]>([]);
  const loading = ref(false);
  const loaded = ref(false);
  const error = ref<string | null>(null);

  async function loadFeeds(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      [feeds.value, inProgress.value] = await Promise.all([fetchPodcastFeeds(), fetchPodcastsInProgress()]);
    } catch (e) {
      error.value = message(e);
    } finally {
      loading.value = false;
      loaded.value = true;
    }
  }

  const feedById = (id: number) => feeds.value.find((f) => f.id === id) ?? null;

  /** Subscribe by address; true when it worked (the reason is shown when not). */
  async function subscribe(url: string): Promise<PodcastFeed | null> {
    try {
      const out = await subscribePodcast(url);
      toasts.push("success", `Subscribed to ${out.feed.title}`, {
        detail: [`${out.episodes_added} episode${out.episodes_added === 1 ? "" : "s"}`, ...out.warnings].join(" · "),
      });
      await loadFeeds();
      void useJobsStore().refresh(); // new episodes may start downloading
      return out.feed;
    } catch (e) {
      toasts.push("error", "Couldn't subscribe", { detail: message(e) });
      return null;
    }
  }

  async function unsubscribe(id: number): Promise<void> {
    await unsubscribePodcast(id);
    upNext.value = upNext.value.filter((e) => e.feed_id !== id);
    if (active.value?.feed.id === id) await leave(false);
    await loadFeeds();
  }

  async function importOpml(text: string): Promise<void> {
    try {
      const r = await importPodcastOpml(text);
      const parts = [`${r.added} added`];
      if (r.already_subscribed) parts.push(`${r.already_subscribed} already subscribed`);
      if (r.invalid.length) parts.push(`${r.invalid.length} not readable`);
      toasts.push("success", "Imported podcasts", { detail: `${parts.join(" · ")}. New ones are read in the background.` });
      await loadFeeds();
      // The feeds are read in the background: show their episodes as they come.
      window.setTimeout(() => void loadFeeds(), 4000);
    } catch (e) {
      toasts.push("error", "Couldn't import that file", { detail: message(e) });
    }
  }

  /** Read one feed again (or every feed); failures show on the feed itself. */
  async function refresh(feedId?: number): Promise<void> {
    try {
      const out = await refreshPodcasts(feedId);
      const added = out.reduce((n, r) => n + r.episodes_added, 0);
      const failed = out.filter((r) => r.error);
      if (failed.length) {
        toasts.push("error", failed.length === 1 ? "A feed couldn't be read" : `${failed.length} feeds couldn't be read`, { detail: failed[0]!.error ?? "" });
      } else {
        toasts.push("info", added ? `${added} new episode${added === 1 ? "" : "s"}` : "No new episodes", { ttl: 2500 });
      }
    } catch (e) {
      toasts.push("error", "Couldn't refresh", { detail: message(e) });
    }
    await loadFeeds();
    if (feedId != null && episodesFeed.value === feedId) await loadEpisodes(feedId);
    void useJobsStore().refresh();
  }

  async function saveSettings(feedId: number, s: PodcastFeedSettings): Promise<void> {
    const f = await savePodcastFeedSettings(feedId, s);
    feeds.value = feeds.value.map((x) => (x.id === f.id ? f : x));
    if (active.value?.feed.id === f.id) {
      active.value = { ...active.value, feed: f };
      if (s.speed !== undefined) await setPlaybackRate(clampSpeed(f.speed));
    }
    if (detail.value?.feed.id === f.id) detail.value = { ...detail.value, feed: f };
  }

  // --- one feed's episodes -----------------------------------------------------------
  const episodes = ref<PodcastEpisode[]>([]);
  const episodesFeed = ref<number | null>(null);
  const unplayedOnly = ref(false);

  async function loadEpisodes(feedId: number): Promise<void> {
    if (episodesFeed.value !== feedId) episodes.value = [];
    episodesFeed.value = feedId;
    error.value = null;
    try {
      if (!feeds.value.length) await loadFeeds();
      episodes.value = await fetchPodcastEpisodes(feedId, unplayedOnly.value);
    } catch (e) {
      error.value = message(e);
    }
  }

  // --- one episode -------------------------------------------------------------------
  const detail = ref<PodcastEpisodeDetail | null>(null);
  const history = ref<AudiobookSession[]>([]);

  async function openEpisode(id: number): Promise<void> {
    error.value = null;
    try {
      detail.value = await fetchPodcastEpisode(id);
      history.value = await fetchPodcastEpisodeHistory(id);
    } catch (e) {
      error.value = message(e);
    }
  }

  /** Every copy of an episode the screens hold, updated together. */
  function patchEpisode(id: number, change: Partial<PodcastEpisode>): void {
    const apply = (e: PodcastEpisode) => (e.id === id ? { ...e, ...change } : e);
    episodes.value = episodes.value.map(apply);
    inProgress.value = inProgress.value.map(apply);
    downloads.value = downloads.value.map(apply);
    upNext.value = upNext.value.map(apply);
    if (detail.value?.id === id) detail.value = { ...detail.value, ...change };
    if (active.value?.episode.id === id) active.value = { ...active.value, episode: { ...active.value.episode, ...change } };
  }

  async function setPlayed(ep: PodcastEpisode, played: boolean): Promise<void> {
    await markPodcastEpisodePlayed(ep.id, played);
    patchEpisode(ep.id, { played_at: played ? Date.now() : null, position_ms: played ? ep.position_ms : 0 });
    await loadFeeds();
    if (unplayedOnly.value && episodesFeed.value != null && played) episodes.value = episodes.value.filter((e) => e.id !== ep.id);
  }

  // --- downloads -----------------------------------------------------------------------
  const downloads = ref<PodcastEpisode[]>([]);
  const folder = ref<PodcastFolder | null>(null);

  async function loadDownloads(): Promise<void> {
    try {
      [downloads.value, folder.value] = await Promise.all([fetchPodcastDownloads(), fetchPodcastFolder()]);
    } catch (e) {
      error.value = message(e);
    }
  }

  /** The running download of each episode (from the server's jobs). */
  const downloadJobs = computed(() => {
    const m = new Map<number, JobInfo>();
    for (const j of useJobsStore().jobs) {
      if (j.kind === "podcast_download" && isJobActive(j) && j.payload) m.set(Number(j.payload), j);
    }
    return m;
  });

  async function download(ep: PodcastEpisode): Promise<void> {
    try {
      await downloadPodcastEpisode(ep.id);
      void useJobsStore().refresh();
    } catch (e) {
      toasts.push("error", "Couldn't download that episode", { detail: message(e) });
    }
  }

  /** Cancel the download, or delete the file (the episode and its place stay). */
  async function removeDownload(ep: PodcastEpisode): Promise<void> {
    try {
      await deletePodcastDownload(ep.id);
      patchEpisode(ep.id, { downloaded: false, file_bytes: null });
      downloads.value = downloads.value.filter((d) => d.id !== ep.id);
      void useJobsStore().refresh();
      if (folder.value) folder.value = await fetchPodcastFolder();
    } catch (e) {
      toasts.push("error", "Couldn't remove the download", { detail: message(e) });
    }
  }

  /** A download finished somewhere: show it. */
  async function downloadsChanged(): Promise<void> {
    if (episodesFeed.value != null) await loadEpisodes(episodesFeed.value);
    if (downloads.value.length || folder.value) await loadDownloads();
    if (detail.value) {
      const d = await fetchPodcastEpisode(detail.value.id).catch(() => null);
      if (d) detail.value = d;
    }
  }

  // --- Up Next -------------------------------------------------------------------------
  const upNext = ref<PodcastEpisode[]>(loadUpNext());
  watch(upNext, (v) => {
    try {
      uiSet(UP_NEXT_KEY, JSON.stringify(v));
    } catch {
      /* the list just will not survive a restart */
    }
  }, { deep: true });

  const inUpNext = (id: number) => upNext.value.some((e) => e.id === id);

  async function addToUpNext(ep: PodcastEpisode): Promise<void> {
    if (inUpNext(ep.id) || active.value?.episode.id === ep.id) return;
    upNext.value = [...upNext.value, ep];
    // While a podcast plays, the queue behind it is Up Next: it plays in turn.
    if (isActive.value) {
      await dropAutoNext();
      await queue.appendTracks([episodeTrack(ep)]);
    }
    toasts.push("success", "Added to Up Next", { detail: ep.title, ttl: 2000 });
  }

  async function removeFromUpNext(id: number): Promise<void> {
    upNext.value = upNext.value.filter((e) => e.id !== id);
    if (!isActive.value) return;
    const at = queue.tracks.findIndex((t, i) => i !== queue.index && episodeOfTrack(t.id) === id);
    if (at >= 0) await queue.removeAt(at);
  }

  async function moveInUpNext(id: number, by: -1 | 1): Promise<void> {
    const i = upNext.value.findIndex((e) => e.id === id);
    const j = i + by;
    if (i < 0 || j < 0 || j >= upNext.value.length) return;
    const next = [...upNext.value];
    [next[i], next[j]] = [next[j]!, next[i]!];
    upNext.value = next;
    if (!isActive.value) return;
    const from = queue.tracks.findIndex((t) => episodeOfTrack(t.id) === id);
    const to = queue.tracks.findIndex((t) => episodeOfTrack(t.id) === next[i]!.id);
    if (from >= 0 && to >= 0 && from !== queue.index && to !== queue.index) await queue.reorder(from, to);
  }

  // --- listening ----------------------------------------------------------------------
  /** The episode the engine is playing (or has paused), with its show. */
  const active = ref<{ episode: PodcastEpisode; feed: PodcastFeed } | null>(null);
  /** The show's next unplayed episode, queued by auto-advance (not shown in Up Next). */
  let autoNext: number | null = null;

  const isActive = computed(() => active.value !== null && episodeOfTrack(player.currentTrack?.id) === active.value.episode.id);
  const offsetMs = computed(() => (isActive.value ? player.positionMs : (active.value?.episode.position_ms ?? 0)));
  const speed = computed(() => active.value?.feed.speed ?? 1);
  const skipBackS = computed(() => active.value?.feed.skip_back_s ?? 15);
  const skipForwardS = computed(() => active.value?.feed.skip_forward_s ?? 30);
  const stash = computed(() => stashStore.stash);

  function stashMusic(): void {
    if (stashStore.stash) return; // already put aside (by a book, or an earlier episode)
    const books = useAudiobooksStore();
    const cur = queue.current;
    if (!cur || isEpisodeTrack(cur.id) || books.active) return; // not music
    stashStore.stash = {
      tracks: queue.tracks.map((t) => ({ ...t })),
      index: queue.index ?? 0,
      positionMs: player.positionMs,
      repeat: player.repeat,
      shuffle: player.shuffle,
      wasPlaying: player.isPlaying,
    };
  }

  async function feedOf(ep: PodcastEpisode): Promise<PodcastFeed> {
    const known = feedById(ep.feed_id);
    if (known) return known;
    return (await fetchPodcastEpisode(ep.id)).feed;
  }

  /** Play a show's newest unplayed episode (a show's Play). */
  async function playLatest(f: PodcastFeed): Promise<void> {
    try {
      const [latest] = await fetchPodcastEpisodes(f.id, true);
      if (latest) await play(latest);
      else toasts.push("info", `Everything in ${f.title} is played`, { ttl: 2500 });
    } catch (e) {
      toasts.push("error", "Couldn't play that show", { detail: message(e) });
    }
  }

  /** Play an episode from where you left off (or `from`); Up Next follows it. */
  async function play(ep: PodcastEpisode, from?: number): Promise<void> {
    const books = useAudiobooksStore();
    if (isActive.value && active.value?.episode.id !== ep.id) await reportNow();
    if (books.active) {
      // A book hands over: its place is saved; the music stays put aside.
      await player.pause();
      await books.reportNow();
    }
    stashMusic();
    const feed = await feedOf(ep);
    upNext.value = upNext.value.filter((e) => e.id !== ep.id);
    active.value = { episode: ep, feed };
    lastSaved = -1;
    autoNext = null;
    await setPlaybackRate(clampSpeed(feed.speed));
    await queuePlayAt([episodeTrack(ep), ...upNext.value.map(episodeTrack)], 0, from ?? startOffset(ep));
    startSaving();
    await queueAutoNext();
  }

  /** With nothing in Up Next and auto-advance on, queue the show's next unplayed episode. */
  async function queueAutoNext(): Promise<void> {
    const a = active.value;
    if (!a || !a.feed.auto_advance || upNext.value.length || autoNext != null) return;
    let list: PodcastEpisode[];
    try {
      list = await fetchPodcastEpisodes(a.feed.id, true);
    } catch {
      return;
    }
    // Newest first: the next one is the one published after this, else the newest left.
    const others = list.filter((e) => e.id !== a.episode.id);
    const after = others.filter((e) => (e.published_at ?? 0) > (a.episode.published_at ?? 0));
    const next = after.length ? after[after.length - 1] : others[0];
    if (!next || !active.value) return;
    autoNext = next.id;
    await queue.appendTracks([episodeTrack(next)]);
  }

  /** Something was added to Up Next: it goes before the auto-advance pick. */
  async function dropAutoNext(): Promise<void> {
    if (autoNext == null) return;
    const id = autoNext;
    autoNext = null;
    const at = queue.tracks.findIndex((t, i) => i !== queue.index && episodeOfTrack(t.id) === id);
    if (at >= 0) await queue.removeAt(at);
  }

  async function seekTo(ms: number): Promise<void> {
    const a = active.value;
    if (!a) return;
    const d = a.episode.duration_ms ?? player.durationMs ?? 0;
    const to = Math.max(0, d > 0 ? Math.min(ms, d) : ms);
    await player.seekTo(to);
    await reportNow(to);
  }

  async function skip(direction: 1 | -1): Promise<void> {
    if (!active.value) return;
    const step = (direction > 0 ? skipForwardS.value : skipBackS.value) * 1000 * direction;
    await seekTo(player.positionMs + step);
  }

  async function setSpeed(v: number): Promise<void> {
    const a = active.value;
    if (!a) return;
    const s = clampSpeed(v);
    await setPlaybackRate(s);
    active.value = { ...a, feed: { ...a.feed, speed: s } };
    try {
      await saveSettings(a.feed.id, { speed: s });
    } catch {
      /* the speed still applies; it just is not remembered */
    }
  }

  // --- saving the position ---------------------------------------------------------------
  let lastSaved = -1;
  let saveTimer: number | undefined;

  async function reportNow(override?: number): Promise<void> {
    const a = active.value;
    if (!a) return;
    const at = Math.round(override ?? offsetMs.value);
    if (at === lastSaved && override === undefined) return;
    lastSaved = at;
    try {
      const out = await savePodcastPosition(a.episode.id, at);
      patchEpisode(a.episode.id, {
        position_ms: out.offset_ms,
        position_updated_at: out.updated_at,
        played_at: out.played ? (a.episode.played_at ?? out.updated_at) : a.episode.played_at,
      });
    } catch {
      lastSaved = -1; // retried by the next tick
    }
  }

  function startSaving(): void {
    window.clearInterval(saveTimer);
    saveTimer = window.setInterval(() => {
      if (isActive.value && player.isPlaying) void reportNow();
    }, POSITION_SAVE_MS);
  }

  watch(
    () => [player.status, player.currentTrack?.id] as const,
    ([status, trackId], [prevStatus, prevTrack]) => {
      const a = active.value;
      if (!a) return;
      const now = episodeOfTrack(trackId);
      const before = episodeOfTrack(prevTrack);
      if (now === a.episode.id && status !== prevStatus) void reportNow();
      // The engine moved on to the next episode in the queue: it is the one now.
      if (now != null && now !== a.episode.id) {
        if (before === a.episode.id) void savePodcastPosition(a.episode.id, a.episode.duration_ms ?? player.durationMs ?? 0).catch(() => undefined);
        void takeUp(now);
        return;
      }
      // Something other than a podcast took over: done being the context.
      if (trackId != null && now == null && status !== "stopped") void leave(false, useAudiobooksStore().partIds.has(trackId));
    },
  );

  /** The engine is now on episode `id` (Up Next or auto-advance moved it on). */
  async function takeUp(id: number): Promise<void> {
    const queued = upNext.value.find((e) => e.id === id);
    let ep: PodcastEpisode;
    let feed: PodcastFeed;
    try {
      const d = await fetchPodcastEpisode(id);
      const { feed: f, ...rest } = d;
      ep = rest;
      feed = f;
    } catch {
      if (!queued) return;
      ep = queued;
      feed = await feedOf(queued);
    }
    upNext.value = upNext.value.filter((e) => e.id !== id);
    if (autoNext === id) autoNext = null;
    active.value = { episode: ep, feed };
    lastSaved = -1;
    await setPlaybackRate(clampSpeed(feed.speed));
    const at = startOffset(ep);
    if (at > 0) await player.seekTo(at);
    await queueAutoNext();
  }

  /** After a restart the engine may still hold an episode: take it up again (without playing). */
  async function adopt(): Promise<void> {
    if (active.value) return;
    const id = episodeOfTrack(player.currentTrack?.id);
    if (id == null) return;
    try {
      const d = await fetchPodcastEpisode(id);
      const { feed, ...ep } = d;
      active.value = { episode: ep, feed };
      await setPlaybackRate(clampSpeed(feed.speed));
      lastSaved = player.positionMs;
      startSaving();
    } catch {
      /* no server: nothing to take up */
    }
  }

  function saveOnClose(): void {
    if (active.value && isActive.value) void reportNow();
  }
  if (typeof window !== "undefined") {
    window.addEventListener("pagehide", saveOnClose);
    window.addEventListener("beforeunload", saveOnClose);
  }

  /**
   * The podcast is no longer the engine's context. `handoff`: a book took
   * over, which keeps the music put aside and sets its own speed.
   */
  async function leave(restoreMusic: boolean, handoff = false): Promise<void> {
    window.clearInterval(saveTimer);
    const had = active.value;
    active.value = null;
    autoNext = null;
    if (handoff) return;
    await setPlaybackRate(1);
    const s = stashStore.stash;
    stashStore.stash = null;
    if (restoreMusic && s) {
      await queueRestore(s.tracks, s.index, s.positionMs, s.repeat, s.shuffle);
      if (s.wasPlaying) await player.resume();
    }
    if (had) void loadFeeds();
  }

  /** Put the episode down (its place saved) and bring the music back where it was. */
  async function returnToMusic(): Promise<void> {
    if (active.value) {
      await player.pause();
      await reportNow();
    }
    await leave(true);
  }

  return {
    feeds,
    inProgress,
    loading,
    loaded,
    error,
    loadFeeds,
    feedById,
    subscribe,
    unsubscribe,
    importOpml,
    refresh,
    saveSettings,
    episodes,
    episodesFeed,
    unplayedOnly,
    loadEpisodes,
    detail,
    history,
    openEpisode,
    setPlayed,
    downloads,
    folder,
    loadDownloads,
    downloadJobs,
    download,
    removeDownload,
    downloadsChanged,
    upNext,
    inUpNext,
    addToUpNext,
    removeFromUpNext,
    moveInUpNext,
    active,
    isActive,
    offsetMs,
    speed,
    skipBackS,
    skipForwardS,
    stash,
    play,
    playLatest,
    seekTo,
    skip,
    setSpeed,
    reportNow,
    adopt,
    leave,
    returnToMusic,
  };
});
