import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import {
  addAudiobookBookmark,
  deleteAudiobookBookmark,
  editAudiobook,
  editAudiobookBookmark,
  enrichAudiobooks,
  fetchAudiobook,
  fetchAudiobookHistory,
  fetchAudiobookListeners,
  addAudiobookListener,
  deleteAudiobookListener,
  fetchAudiobooks,
  fetchTrack,
  markAudiobookFinished,
  saveAudiobookPosition,
  saveAudiobookSettings,
  dismissAudiobookFromShelf,
  setAudiobookListener,
  type AudiobookListener,
  type AudiobookMetaEdit,
  type AudiobookQuery,
} from "../api";
import {
  bookOffset,
  chapterIndexAt,
  clampSpeed,
  nextChapterStart,
  prevChapterStart,
  resolveOffset,
  skipTarget,
  sleepGain,
  SKIP_DEFAULTS,
  SLEEP_FADE_MS,
  startOffset,
  wallMs,
} from "../lib/audiobook";
import { queuePlayAt, queueRestore, setPlaybackRate } from "../tauri";
import type {
  Audiobook,
  AudiobookBookmark,
  AudiobookDetail,
  AudiobookSession,
  Track,
} from "../types";
import { useJobsStore } from "./jobs";
import { useMusicStashStore, type MusicStash } from "./musicStash";
import { usePodcastsStore } from "./podcasts";
import { isEpisodeTrack } from "../lib/podcast";
import { usePlayerStore } from "./player";
import { useQueueStore } from "./queue";
import { useToastsStore } from "./toasts";

/** How often a playing book saves its position (also saved on pause, stop, seek and part change). */
export const POSITION_SAVE_MS = 10_000;

/** The music that was playing when a book took over, kept so it can come back exactly. */

export type SleepTimer =
  | { kind: "minutes"; minutes: number; endsAt: number }
  | { kind: "chapter"; endOffsetMs: number };

/**
 * Audiobooks: the library, the book that is playing, and everything that
 * follows the listener (position, bookmarks, history, speed, sleep timer).
 *
 * A position is always a *book offset* (ms from the start of the book); the
 * engine plays the book's parts as an ordinary queue, and this store maps
 * between the two. Music and a book never interleave: starting a book puts
 * the music queue aside (not destroyed), and coming back restores it.
 */
export const useAudiobooksStore = defineStore("audiobooks", () => {
  const player = usePlayerStore();
  const queue = useQueueStore();
  const toasts = useToastsStore();

  // --- listener ----------------------------------------------------------------
  // Several people can share one server's books, each with their own place,
  // bookmarks, history, speed and finished flags. There are no accounts: the
  // name is just remembered here and sent with every audiobook request.
  const LISTENER_KEY = "kahawai.audiobook-listener";
  const readListener = (): string => {
    try {
      return localStorage.getItem(LISTENER_KEY) ?? "";
    } catch {
      return "";
    }
  };
  /** "" is the server's default listener. */
  const listener = ref(readListener());
  setAudiobookListener(listener.value);
  const listeners = ref<AudiobookListener[]>([]);

  async function loadListeners(): Promise<void> {
    listeners.value = await fetchAudiobookListeners();
  }

  /** A new listener, who this Player then listens as. */
  async function addListener(name: string): Promise<void> {
    const made = await addAudiobookListener(name);
    await loadListeners();
    await switchListener(made.name);
  }

  /** Forget a listener and their progress; if it was this Player's, back to Default. */
  async function removeListener(id: number): Promise<void> {
    const gone = listeners.value.find((l) => l.id === id);
    await deleteAudiobookListener(id);
    if (gone && (gone.name === listener.value || (id === 0 && !listener.value))) await switchListener("");
    await loadListeners();
  }

  /** Listen as someone else: the book in hand is put down (position saved) first. */
  async function switchListener(name: string): Promise<void> {
    const next = name.trim() === "Default" ? "" : name.trim();
    if (next === listener.value) return;
    if (active.value) {
      await player.pause();
      await reportNow();
      await leave(true);
    }
    listener.value = next;
    setAudiobookListener(next);
    try {
      if (next) localStorage.setItem(LISTENER_KEY, next);
      else localStorage.removeItem(LISTENER_KEY);
    } catch {
      /* private mode: the choice just will not survive a restart */
    }
    detail.value = null;
    history.value = [];
    books.value = [];
    shelf.value = [];
    await loadLibrary();
  }

  // --- library ---------------------------------------------------------------
  const books = ref<Audiobook[]>([]);
  const shelf = ref<Audiobook[]>([]);
  const query = ref<{ q: string; author: string; series: string; finished: "all" | "yes" | "no" }>({
    q: "",
    author: "",
    series: "",
    finished: "all",
  });
  const loading = ref(false);
  /** The library has been loaded at least once (so a catalog update should refresh it). */
  const loaded = ref(false);
  const error = ref<string | null>(null);

  const authors = computed(() => [...new Set(books.value.map((b) => b.author).filter((a): a is string => !!a))].sort((a, b) => a.localeCompare(b)));
  const seriesNames = computed(() => [...new Set(books.value.map((b) => b.series).filter((a): a is string => !!a))].sort((a, b) => a.localeCompare(b)));

  function apiQuery(): AudiobookQuery {
    const q = query.value;
    return {
      q: q.q || undefined,
      author: q.author || undefined,
      series: q.series || undefined,
      finished: q.finished === "all" ? undefined : q.finished === "yes",
    };
  }

  async function loadLibrary(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      [books.value, shelf.value] = await Promise.all([fetchAudiobooks(apiQuery()), fetchAudiobooks({ shelf: "continue" })]);
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
      loaded.value = true;
    }
  }

  /** Take a book off the Continue listening shelf (its place is kept; playing it puts it back). */
  async function dismissFromShelf(id: number): Promise<void> {
    const before = shelf.value;
    shelf.value = before.filter((b) => b.id !== id);
    try {
      await dismissAudiobookFromShelf(id);
    } catch (e) {
      shelf.value = before;
      toasts.push("error", "Couldn't remove the book from Continue listening", { detail: e instanceof Error ? e.message : String(e) });
    }
  }

  /** Books you could continue: started, not finished. */
  const continueShelf = computed(() => shelf.value.filter((b) => !b.finished_at));

  // --- detail ----------------------------------------------------------------
  const detail = ref<AudiobookDetail | null>(null);
  const history = ref<AudiobookSession[]>([]);

  async function openDetail(id: number): Promise<void> {
    error.value = null;
    try {
      detail.value = await fetchAudiobook(id);
      history.value = await fetchAudiobookHistory(id);
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  async function refreshDetail(): Promise<void> {
    const id = detail.value?.id ?? active.value?.id;
    if (id == null) return;
    const fresh = await fetchAudiobook(id);
    if (detail.value?.id === id) detail.value = fresh;
    if (active.value?.id === id) active.value = { ...active.value, ...fresh, parts: active.value.parts, chapters: fresh.chapters };
    history.value = await fetchAudiobookHistory(id);
  }

  // --- the playing book ------------------------------------------------------
  /** The book the engine is playing (or has paused), with its parts as tracks. */
  const active = ref<AudiobookDetail | null>(null);
  let partTracks: Track[] = [];
  // Shared with the podcast context (stores/musicStash).
  const stashStore = useMusicStashStore();
  const stash = computed<MusicStash | null>({
    get: () => stashStore.stash,
    set: (v) => (stashStore.stash = v),
  });

  const partIds = computed(() => new Set(active.value?.parts.map((p) => p.track_id) ?? []));
  /** A book (not music) is what the engine has loaded. */
  const isActive = computed(() => active.value !== null && player.currentTrack !== null && partIds.value.has(player.currentTrack.id));

  /** Where the listener is in the book, ms. */
  const offsetMs = computed(() => {
    const a = active.value;
    if (!a) return 0;
    return bookOffset(a.parts, player.currentTrack?.id, player.positionMs) ?? a.position_ms;
  });
  const chapterIndex = computed(() => (active.value ? chapterIndexAt(active.value.chapters, offsetMs.value) : -1));
  const chapter = computed(() => (active.value && chapterIndex.value >= 0 ? active.value.chapters[chapterIndex.value] : null));
  const speed = computed(() => active.value?.settings.speed ?? 1);
  const skipBackS = computed(() => active.value?.settings.skip_back_s ?? SKIP_DEFAULTS.back);
  const skipForwardS = computed(() => active.value?.settings.skip_forward_s ?? SKIP_DEFAULTS.forward);

  async function tracksFor(book: AudiobookDetail): Promise<Track[]> {
    return Promise.all(book.parts.map((p) => fetchTrack(p.track_id)));
  }

  function stashMusic(): void {
    // Music already put aside stays put: a second book must not overwrite it
    // with the first book's parts.
    if (stash.value || isActive.value || isEpisodeTrack(queue.current?.id)) return;
    const tracks = queue.tracks.map((t) => ({ ...t }));
    if (tracks.length === 0) return;
    stash.value = {
      tracks,
      index: queue.index ?? 0,
      positionMs: player.positionMs,
      repeat: player.repeat,
      shuffle: player.shuffle,
      wasPlaying: player.isPlaying,
    };
  }

  /**
   * Play a book from `fromOffset` (default: where you left off, or the start
   * of a finished one). The music queue, if any, is put aside.
   */
  async function start(id: number, fromOffset?: number): Promise<void> {
    const book = detail.value?.id === id ? detail.value : await fetchAudiobook(id);
    if (book.parts.length === 0) {
      toasts.push("error", "This book has no playable files");
      return;
    }
    if (isActive.value && active.value && active.value.id !== id) await reportNow();
    const podcasts = usePodcastsStore();
    if (podcasts.active) {
      // An episode hands over: its place is saved; the music stays put aside.
      await player.pause();
      await podcasts.reportNow();
    }
    stashMusic();
    const tracks = await tracksFor(book);
    partTracks = tracks;
    const at = fromOffset ?? startOffset(book.position_ms, book.duration_ms);
    const r = resolveOffset(book.parts, at);
    if (!r) return;
    active.value = { ...book };
    await setPlaybackRate(clampSpeed(book.settings.speed));
    await queuePlayAt(tracks, r.partIndex, r.trackOffsetMs);
    lastSaved = at;
    startSaving();
  }

  /**
   * After a restart the engine reloads the queue it had, which may be a
   * book's parts. Work out whether it is one, and if so take it up as the
   * playing book (without starting it), so the controls, whole-book seek bar
   * and position saving carry on. Looks at the most recently played books.
   */
  async function adopt(): Promise<void> {
    if (active.value) return;
    const current = player.currentTrack?.id;
    if (current == null) return;
    let candidates: Audiobook[];
    try {
      candidates = await fetchAudiobooks({ shelf: "continue" });
    } catch {
      return; // no server: nothing to adopt
    }
    for (const c of candidates.slice(0, 5)) {
      const book = await fetchAudiobook(c.id);
      if (book.parts.some((p) => p.track_id === current)) {
        active.value = { ...book };
        await setPlaybackRate(clampSpeed(book.settings.speed));
        lastSaved = offsetMs.value;
        startSaving();
        return;
      }
    }
  }

  /** Jump to a book offset, across parts if need be. */
  async function seekToOffset(ms: number): Promise<void> {
    const a = active.value;
    if (!a) return;
    const target = Math.min(Math.max(0, ms), a.duration_ms);
    const r = resolveOffset(a.parts, target);
    if (!r) return;
    if (player.currentTrack?.id === r.trackId) {
      await player.seekTo(r.trackOffsetMs);
    } else {
      // A book taken up after a restart has not fetched its parts yet.
      if (partTracks.length === 0) partTracks = await tracksFor(a);
      await queuePlayAt(partTracks, r.partIndex, r.trackOffsetMs);
    }
    await reportNow(target);
  }

  async function skip(direction: 1 | -1): Promise<void> {
    const a = active.value;
    if (!a) return;
    const step = (direction > 0 ? skipForwardS.value : skipBackS.value) * 1000 * direction;
    await seekToOffset(skipTarget(offsetMs.value, step, a.duration_ms));
  }

  async function nextChapter(): Promise<void> {
    const a = active.value;
    if (!a) return;
    const to = nextChapterStart(a.chapters, offsetMs.value);
    if (to != null) await seekToOffset(to);
  }

  async function previousChapter(): Promise<void> {
    const a = active.value;
    if (!a) return;
    await seekToOffset(prevChapterStart(a.chapters, offsetMs.value));
  }

  // --- saving the position ---------------------------------------------------
  let lastSaved = -1;
  let saveTimer: number | undefined;

  /** Save the position now (`override` when the engine has not caught up with a seek yet). */
  async function reportNow(override?: number): Promise<void> {
    const a = active.value;
    if (!a) return;
    const at = Math.round(override ?? offsetMs.value);
    if (at === lastSaved && override === undefined) return;
    lastSaved = at;
    try {
      const out = await saveAudiobookPosition(a.id, at);
      a.position_ms = out.book_offset_ms;
      a.finished_at = out.finished ? (a.finished_at ?? Date.now()) : null;
      a.progress = a.duration_ms > 0 ? out.book_offset_ms / a.duration_ms : 0;
      if (detail.value?.id === a.id) {
        detail.value.position_ms = a.position_ms;
        detail.value.finished_at = a.finished_at;
        detail.value.progress = a.progress;
      }
    } catch {
      // A failed save is retried by the next tick; the position is not lost.
      lastSaved = -1;
    }
  }

  function startSaving(): void {
    window.clearInterval(saveTimer);
    saveTimer = window.setInterval(() => {
      if (isActive.value && player.isPlaying) void reportNow();
    }, POSITION_SAVE_MS);
  }

  // Save on pause / stop / a new part (the spec's "pause, stop, seek, close"
  // moments other than the timer).
  watch(
    () => [player.status, player.currentTrack?.id] as const,
    ([status, trackId], [prevStatus, prevTrack]) => {
      if (!active.value) return;
      if (partIds.value.has(trackId ?? -1) || partIds.value.has(prevTrack ?? -1)) {
        if (status !== prevStatus || trackId !== prevTrack) void reportNow();
      }
      // Something other than this book took over the engine: the book is
      // done being the context.
      // A podcast taking over is a hand-over: the music stays put aside and
      // the podcast sets its own speed.
      if (trackId != null && !partIds.value.has(trackId) && status !== "stopped") void leave(false, isEpisodeTrack(trackId));
    },
  );

  /** App closing or the page going away: one last save. */
  function saveOnClose(): void {
    const a = active.value;
    if (!a || !isActive.value) return;
    // keepalive lets the request outlive the page.
    void reportNow();
  }
  if (typeof window !== "undefined") {
    window.addEventListener("pagehide", saveOnClose);
    window.addEventListener("beforeunload", saveOnClose);
  }

  // --- leaving, and music coming back ----------------------------------------
  /**
   * The book is no longer the engine's context: speed back to normal, stop
   * saving. `handoff`: a podcast took over, which keeps the music put aside
   * and sets its own speed.
   */
  async function leave(restoreMusic: boolean, handoff = false): Promise<void> {
    window.clearInterval(saveTimer);
    cancelSleep(true);
    const had = active.value;
    active.value = null;
    partTracks = [];
    if (handoff) {
      if (had) void loadLibrary();
      return;
    }
    await setPlaybackRate(1);
    const s = stash.value;
    stash.value = null;
    if (restoreMusic && s) {
      await queueRestore(s.tracks, s.index, s.positionMs, s.repeat, s.shuffle);
      if (s.wasPlaying) await player.resume();
    }
    if (had) void loadLibrary();
  }

  /** Put the book down (position saved) and bring the music back where it was. */
  async function returnToMusic(): Promise<void> {
    if (active.value) {
      await player.pause();
      await reportNow();
    }
    await leave(true);
  }

  // --- speed and skip settings -------------------------------------------------
  /**
   * Set a book's speed (the playing book's by default). The engine changes
   * only for the playing book; the playing copy and the detail copy of the
   * book both show the new speed.
   */
  async function setSpeed(v: number, id = active.value?.id ?? detail.value?.id): Promise<void> {
    if (id == null) return;
    const speedNow = clampSpeed(v);
    if (active.value?.id === id) await setPlaybackRate(speedNow);
    for (const b of [active.value, detail.value]) {
      if (b?.id === id) b.settings = { ...b.settings, speed: speedNow };
    }
    try {
      await saveAudiobookSettings(id, { speed: speedNow });
    } catch {
      /* the speed still applies; it just is not remembered */
    }
  }

  async function setSkips(backS: number, forwardS: number): Promise<void> {
    const a = active.value ?? detail.value;
    if (!a) return;
    a.settings = { ...a.settings, skip_back_s: backS, skip_forward_s: forwardS };
    if (active.value && detail.value && active.value.id === detail.value.id) detail.value.settings = a.settings;
    await saveAudiobookSettings(a.id, { skip_back_s: backS, skip_forward_s: forwardS });
  }

  // --- bookmarks ---------------------------------------------------------------
  async function addBookmark(name?: string): Promise<AudiobookBookmark | null> {
    const a = active.value;
    if (!a) return null;
    const bm = await addAudiobookBookmark(a.id, offsetMs.value, name);
    a.bookmarks = [...a.bookmarks, bm].sort((x, y) => x.book_offset_ms - y.book_offset_ms);
    if (detail.value?.id === a.id) detail.value.bookmarks = a.bookmarks;
    toasts.push("success", "Bookmark added", { detail: bm.name, ttl: 2500 });
    return bm;
  }

  async function renameBookmark(id: number, name: string): Promise<void> {
    const book = detail.value ?? active.value;
    if (!book) return;
    const bm = await editAudiobookBookmark(book.id, id, { name });
    for (const b of [detail.value, active.value]) {
      if (b?.id === book.id) b.bookmarks = b.bookmarks.map((x) => (x.id === id ? bm : x));
    }
  }

  async function removeBookmark(id: number): Promise<void> {
    const book = detail.value ?? active.value;
    if (!book) return;
    await deleteAudiobookBookmark(book.id, id);
    for (const b of [detail.value, active.value]) {
      if (b?.id === book.id) b.bookmarks = b.bookmarks.filter((x) => x.id !== id);
    }
  }

  /** Play a book from a bookmark, or jump to it if the book is already playing. */
  async function playFrom(bookId: number, offset: number): Promise<void> {
    if (active.value?.id === bookId && isActive.value) await seekToOffset(offset);
    else await start(bookId, offset);
  }

  // --- finishing, editing --------------------------------------------------------
  async function setFinished(id: number, finished: boolean): Promise<void> {
    const b = await markAudiobookFinished(id, finished);
    if (detail.value?.id === id) detail.value.finished_at = b.finished_at;
    if (active.value?.id === id) active.value.finished_at = b.finished_at;
    await loadLibrary();
  }

  /** Look up a book's missing author, year and cover online. The server says
   *  why not when the lookup is switched off. */
  async function lookUpOnline(id: number): Promise<void> {
    try {
      await enrichAudiobooks(id);
      void useJobsStore().refresh();
      toasts.push("info", "Looking up this book online", { ttl: 3000 });
    } catch (e) {
      toasts.push("error", "Could not look up this book", { detail: e instanceof Error ? e.message : String(e) });
    }
  }

  /** The Info or Edit details dialog opened from a book's menu, with the book in full. */
  const bookDialog = ref<{ kind: "info" | "edit"; book: AudiobookDetail } | null>(null);

  async function showBookDialog(kind: "info" | "edit", id: number): Promise<void> {
    try {
      bookDialog.value = { kind, book: await fetchAudiobook(id) };
    } catch (e) {
      toasts.push("error", "Could not load this book", { detail: e instanceof Error ? e.message : String(e) });
    }
  }

  async function editMeta(id: number, edit: AudiobookMetaEdit): Promise<void> {
    await editAudiobook(id, edit);
    if (detail.value?.id === id) await openDetail(id);
    await loadLibrary();
  }

  // --- sleep timer -------------------------------------------------------------
  const sleep = ref<SleepTimer | null>(null);
  const sleepRemainingMs = ref<number | null>(null);
  let sleepTimer: number | undefined;
  let volumeBefore: number | null = null;

  function restoreVolume(): void {
    if (volumeBefore !== null) {
      void player.changeVolume(volumeBefore);
      volumeBefore = null;
    }
  }

  /** Wall-clock ms left on the timer. */
  function sleepLeft(): number | null {
    const s = sleep.value;
    if (!s) return null;
    if (s.kind === "minutes") return Math.max(0, s.endsAt - Date.now());
    return wallMs(s.endOffsetMs - offsetMs.value, speed.value);
  }

  function tickSleep(): void {
    const left = sleepLeft();
    sleepRemainingMs.value = left;
    if (left === null) return;
    if (left <= SLEEP_FADE_MS) {
      if (volumeBefore === null) volumeBefore = player.volume;
      void player.changeVolume(volumeBefore * sleepGain(left));
    }
    if (left <= 0) {
      const restore = volumeBefore;
      cancelSleep(false);
      void (async () => {
        await player.pause();
        // Back to the listener's volume only after the pause has landed, so
        // nothing blips at full volume.
        if (restore !== null) await player.changeVolume(restore);
        await reportNow();
      })();
    }
  }

  function startSleepMinutes(minutes: number): void {
    cancelSleep(true);
    sleep.value = { kind: "minutes", minutes, endsAt: Date.now() + minutes * 60_000 };
    sleepTimer = window.setInterval(tickSleep, 250);
    tickSleep();
  }

  /** Sleep at the end of the chapter being heard. */
  function startSleepEndOfChapter(): void {
    const c = chapter.value;
    const a = active.value;
    if (!a) return;
    const end = c ? c.start_offset_ms + c.duration_ms : a.duration_ms;
    cancelSleep(true);
    sleep.value = { kind: "chapter", endOffsetMs: end };
    sleepTimer = window.setInterval(tickSleep, 250);
    tickSleep();
  }

  /** Stop the timer. `restore`: put the volume back if it was fading. */
  function cancelSleep(restore: boolean): void {
    window.clearInterval(sleepTimer);
    sleepTimer = undefined;
    sleep.value = null;
    sleepRemainingMs.value = null;
    if (restore) restoreVolume();
    else volumeBefore = null;
  }

  return {
    books,
    partIds,
    shelf,
    continueShelf,
    query,
    loading,
    loaded,
    error,
    listener,
    listeners,
    loadListeners,
    addListener,
    removeListener,
    switchListener,
    authors,
    seriesNames,
    loadLibrary,
    detail,
    history,
    openDetail,
    refreshDetail,
    dismissFromShelf,
    bookDialog,
    showBookDialog,
    active,
    isActive,
    stash,
    offsetMs,
    chapterIndex,
    chapter,
    speed,
    skipBackS,
    skipForwardS,
    start,
    adopt,
    seekToOffset,
    skip,
    nextChapter,
    previousChapter,
    reportNow,
    returnToMusic,
    leave,
    setSpeed,
    setSkips,
    addBookmark,
    renameBookmark,
    removeBookmark,
    playFrom,
    setFinished,
    lookUpOnline,
    editMeta,
    sleep,
    sleepRemainingMs,
    startSleepMinutes,
    startSleepEndOfChapter,
    cancelSleep,
  };
});
