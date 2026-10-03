import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import type { Component } from "vue";
import { beforeEach, describe, expect, it } from "vitest";
import { makeState, makeTrack, mockFetch } from "../test/fixtures";
import { $$, menuItems, mountApp, openMenu, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useAudiobooksStore } from "../stores/audiobooks";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import { useViewPrefsStore } from "../stores/viewPrefs";
import type { Audiobook, AudiobookDetail } from "../types";
import AudiobookControls from "./AudiobookControls.vue";
import AudiobookDetailView from "./AudiobookDetail.vue";
import AudiobooksView from "./AudiobooksView.vue";
import NowPlayingView from "./NowPlayingView.vue";
import SeekBar from "./SeekBar.vue";
import Sidebar from "./Sidebar.vue";

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

const summary = (id: number, over: Partial<Audiobook> = {}): Audiobook => ({
  id,
  root_id: 1,
  title: `Book ${id}`,
  author: id === 1 ? "Ann Author" : "Bob Writer",
  narrator: null,
  series: null,
  series_index: null,
  year: null,
  cover_hash: null,
  duration_ms: 36_000_000,
  added_at: 0,
  finished_at: null,
  position_ms: 0,
  last_played_at: null,
  progress: 0,
  path: `/books/Book ${id}`,
  format: "mp3",
  bitrate: 64,
  sample_rate: 44100,
  channels: 1,
  ...over,
});

const detail = (over: Partial<AudiobookDetail> = {}): AudiobookDetail => ({
  ...summary(1, { position_ms: 3_600_000, last_played_at: 5, progress: 0.1 }),
  narrator: "Nora Narrator",
  series: "Saga",
  series_index: 2,
  parts: [{ id: 1, track_id: 101, part_index: 0, title: "One", start_offset_ms: 0, duration_ms: 36_000_000, format: "mp3", bitrate: 64, sample_rate: 44100, channels: 1 }],
  chapters: [
    { id: 1, part_id: 1, title: "Opening", start_offset_ms: 0, duration_ms: 1_000_000 },
    { id: 2, part_id: 1, title: "The Middle", start_offset_ms: 1_000_000, duration_ms: 2_000_000 },
  ],
  bookmarks: [{ id: 9, book_id: 1, book_offset_ms: 120_000, name: "Great line", note: "", created_at: 1 }],
  settings: { speed: 1.25, skip_back_s: 15, skip_forward_s: 30 },
  ...over,
});

beforeEach(() => {
  localStorage.clear();
  tauri.reset();
});

describe("Audiobooks library view", () => {
  function routes(list: Audiobook[] = [summary(1), summary(2)], shelf: Audiobook[] = []) {
    return mockFetch({
      "/api/audiobooks": (url: string) => json(url.includes("shelf=continue") ? shelf : list),
    });
  }

  it("shows a card per book and opens one", async () => {
    routes();
    const { wrapper } = mountApp(AudiobooksView);
    await settle();
    const cards = wrapper.findAll('[data-testid="book-card"]');
    expect(cards).toHaveLength(2);
    expect(cards[0].text()).toContain("Book 1");
    expect(cards[0].text()).toContain("Ann Author");
    expect(cards[0].find('[data-placeholder="book"]').exists()).toBe(true); // no cover: an open book, not a note
    await cards[1].trigger("click");
    expect(useNavStore().view).toEqual({ name: "audiobook", id: 2 });
  });

  it("switches to a list, with progress and length per book, and remembers the choice", async () => {
    routes([summary(1, { progress: 0.42, series: "Saga", series_index: 2 }), summary(2, { finished_at: 5 })]);
    const { wrapper } = mountApp(AudiobooksView);
    await settle();
    await wrapper.get('button[aria-label="List"]').trigger("click");
    await settle();
    expect(useViewPrefsStore().prefs.audiobooksLayout).toBe("list");
    expect(wrapper.find('[data-testid="book-card"]').exists()).toBe(false);
    const rows = wrapper.findAll('[data-testid="book-row"]');
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("Ann Author · Saga #2");
    expect(rows[0].text()).toContain("42%");
    expect(rows[0].text()).toContain("10 h");
    expect(rows[1].text()).toContain("Finished");
    expect(rows[0].get('[data-testid="book-row-format"]').text()).toBe("MP3 · 44.1 kHz · 64 kbps · mono");
    await rows[1].trigger("click");
    expect(useNavStore().view).toEqual({ name: "audiobook", id: 2 });
  });

  it("a book's right-click menu plays it, shows its info and edits its details", async () => {
    const calls = mockFetch({
      "/api/audiobooks/1/history": () => json([]),
      "/api/audiobooks/1": (_u: string, init?: RequestInit) => (init?.method === "PATCH" ? json(summary(1)) : json(detail())),
      "/api/audiobooks": (url: string) => json(url.includes("shelf=continue") ? [] : [summary(1), summary(2)]),
    });
    const { wrapper } = mountApp(AudiobooksView);
    await settle();
    const rightClick = async () => {
      wrapper.get('[data-testid="book-card"]').element.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
      await settle();
    };
    await rightClick();
    expect(menuItems().map((e) => e.textContent?.trim())).toEqual(["Play", "Info", "Edit details"]);

    menuItems()[1].click();
    await settle();
    const info = document.body.querySelector('[data-testid="book-info"]')!;
    expect(info.textContent).toContain("Book 1");
    expect(document.body.querySelector('[data-testid="book-info-Format"]')?.textContent?.trim()).toBe("MP3 · 44.1 kHz · 64 kbps · mono");
    expect(document.body.querySelector('[data-testid="book-info-Folder"]')?.textContent?.trim()).toBe("/books/Book 1");
    expect(document.body.querySelector('[data-testid="book-info-Narrator"]')?.textContent?.trim()).toBe("Nora Narrator");
    useAudiobooksStore().bookDialog = null;
    await settle();

    await rightClick();
    menuItems()[2].click();
    await settle();
    await typeInto(document.body.querySelector('[data-testid="edit-narrator"]') as HTMLInputElement, "New Voice");
    (document.body.querySelector('[data-testid="edit-save"]') as HTMLElement).click();
    await settle();
    const patch = calls.find((c) => c.url.endsWith("/api/audiobooks/1") && c.init?.method === "PATCH");
    expect(JSON.parse(String(patch?.init?.body)).narrator).toBe("New Voice");
  });

  it("has a Continue listening shelf with a progress bar, and hides finished books from it", async () => {
    const started = summary(1, { progress: 0.4, last_played_at: 9, position_ms: 14_400_000 });
    routes([started, summary(2)], [started, summary(3, { finished_at: 7, progress: 1, last_played_at: 8 })]);
    const { wrapper } = mountApp(AudiobooksView);
    await settle();
    const shelf = wrapper.get('[data-testid="continue-shelf"]');
    expect(shelf.findAll('[data-testid="book-card"]')).toHaveLength(1);
    expect(shelf.get('[data-testid="book-progress"]').html()).toContain("width: 40%");
  });

  it("searches by title, author or narrator through the server", async () => {
    const calls = routes();
    const { wrapper } = mountApp(AudiobooksView);
    await settle();
    await typeInto(wrapper.get<HTMLInputElement>('[data-testid="book-search"]').element, "nora");
    await new Promise((r) => setTimeout(r, 260));
    await settle();
    expect(calls.some((c) => c.url.includes("q=nora"))).toBe(true);
  });

  it("shows a running audiobook scan with its file count while the library fills in", async () => {
    mockFetch({
      "/api/jobs": () => json([{ id: "job-4", kind: "scan", label: "Audiobook scan", status: "running", progress: 0.5, message: null, payload: null, files: { done: 50, total: 100 } }]),
      "/api/audiobooks": () => json([]),
    });
    const { wrapper } = mountApp(AudiobooksView, {}, {}, () => {});
    await (await import("../stores/jobs")).useJobsStore().refresh();
    await settle();
    const banner = wrapper.get('[data-testid="books-working"]');
    expect(banner.text()).toContain("Scanning audiobooks");
    expect(banner.text()).toContain("50 of about 100 files");
  });

  it("explains an empty library", async () => {
    routes([], []);
    const { wrapper } = mountApp(AudiobooksView);
    await settle();
    expect(wrapper.text()).toContain("No audiobooks yet");
  });

  it("is in the sidebar", async () => {
    const { wrapper } = mountApp(Sidebar);
    expect(wrapper.text()).toContain("Audiobooks");
    const btn = wrapper.findAll("button").find((b) => b.text().includes("Audiobooks"))!;
    await btn.trigger("click");
    expect(useNavStore().view.name).toBe("audiobooks");
  });
});

describe("Audiobook detail", () => {
  function routes(d = detail()) {
    return mockFetch({
      "/api/audiobooks/1/history": () =>
        json([
          { id: 2, started_at: new Date(2026, 8, 30, 21).getTime(), ended_at: 0, start_offset_ms: 3_000_000, end_offset_ms: 3_600_000, listened_ms: 600_000 },
        ]),
      "/api/audiobooks/1/bookmarks/9": (_u: string, init?: RequestInit) => (init?.method === "DELETE" ? new Response(null, { status: 204 }) : json({ ...d.bookmarks[0], ...JSON.parse(String(init?.body)) })),
      "/api/audiobooks/1/settings": (_u: string, init?: RequestInit) => json({ ...d.settings, ...JSON.parse(String(init?.body)) }),
      "/api/audiobooks/1": (_u: string, init?: RequestInit) => (init?.method === "PATCH" ? json(summary(1)) : json(d)),
      "/api/tracks/101": () => json(makeTrack({ id: 101, duration_ms: 36_000_000 })),
      "/api/audiobooks": () => json([]),
    });
  }

  it("shows the book, where you are, chapters, bookmarks and history", async () => {
    routes();
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await settle();
    expect(wrapper.get('[data-testid="book-title"]').text()).toBe("Book 1");
    expect(wrapper.get('[data-testid="book-by"]').text()).toContain("read by Nora Narrator");
    expect(wrapper.get('[data-testid="book-length"]').text()).toContain("7 h 12 min left"); // 9 h to go at 1.25x
    expect(wrapper.get('[data-testid="play-book"]').text()).toContain("Continue from 1:00:00");
    expect(wrapper.findAll('[data-testid="chapter"]')).toHaveLength(2);
    expect(wrapper.findAll('[data-testid="chapter-format"]').map((e) => e.text())).toEqual(["MP3 · 44.1 kHz · 64 kbps · mono", "MP3 · 44.1 kHz · 64 kbps · mono"]);
    expect(wrapper.get('[data-testid="bookmark"]').text()).toContain("Great line");
    expect(wrapper.get('[data-testid="history-day"]').text()).toContain("stopped at 1:00:00, 10 min listened");
  });

  it("has the same breadcrumb as the other sections: Audiobooks › the book", async () => {
    routes();
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 }, {}, () => {
      useNavStore().go("audiobooks");
      useNavStore().go("audiobook", 1);
    });
    await settle();
    expect(wrapper.get('[data-testid="crumb-current"]').text()).toBe("Book 1");
    expect(useNavStore().trail.at(-1)?.label).toBe("Book 1");
    await wrapper.get('[data-testid="crumb"]').trigger("click");
    expect(useNavStore().view).toEqual({ name: "audiobooks", id: undefined });
  });

  it("Continue starts the book at the saved place with its speed", async () => {
    routes();
    tauri.on("get_state", makeState({ status: "stopped", track: null }));
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await usePlayerStore().init();
    await settle();
    await wrapper.get('[data-testid="play-book"]').trigger("click");
    await settle();
    expect(tauri.callsTo("set_playback_rate").at(-1)).toEqual({ rate: 1.25 });
    const play = tauri.callsTo("queue_play_at").at(-1) as { positionMs: number };
    expect(play.positionMs).toBe(3_600_000);
  });

  it("a chapter or a bookmark starts the book there", async () => {
    routes();
    tauri.on("get_state", makeState({ status: "stopped", track: null }));
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await usePlayerStore().init();
    await settle();
    await wrapper.findAll('[data-testid="chapter"]')[1].trigger("click");
    await settle();
    expect((tauri.callsTo("queue_play_at").at(-1) as { positionMs: number }).positionMs).toBe(1_000_000);
    await wrapper.get('[data-testid="bookmark"]').trigger("click");
    await settle();
    expect((tauri.callsTo("queue_play_at").at(-1) as { positionMs: number }).positionMs).toBe(120_000);
  });

  it("deletes a bookmark and marks the book finished", async () => {
    const calls = routes();
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await settle();
    await wrapper.get('[data-testid="delete-bookmark"]').trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/bookmarks/9") && c.init?.method === "DELETE")).toBe(true);
    expect(useAudiobooksStore().detail?.bookmarks).toHaveLength(0);
    await wrapper.get('[data-testid="toggle-finished"]').trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/finished") && JSON.parse(String(c.init?.body)).finished === true)).toBe(true);
  });

  it("looks a book up online when something is missing, and relays the server's refusal", async () => {
    const calls = mockFetch({
      "/api/audiobooks/enrich": () => json({ error: "online lookup is off: turn on Album info in the Server settings first" }, 400),
      "/api/audiobooks/1/history": () => json([]),
      "/api/audiobooks/1": () => json(detail({ author: null, year: null, cover_hash: null })),
    });
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await settle();
    await wrapper.get('[data-testid="look-up"]').trigger("click");
    await settle();
    const post = calls.find((c) => c.url.endsWith("/api/audiobooks/enrich"));
    expect(JSON.parse(String(post?.init?.body))).toEqual({ book_id: 1 });
    const toasts = (await import("../stores/toasts")).useToastsStore();
    expect(toasts.toasts.at(-1)?.detail).toContain("online lookup is off");
  });

  it("offers no lookup when the author, year and cover are all known", async () => {
    mockFetch({
      "/api/audiobooks/1/history": () => json([]),
      "/api/audiobooks/1": () => json(detail({ author: "A", year: 2000, cover_hash: "h" })),
    });
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await settle();
    expect(wrapper.find('[data-testid="look-up"]').exists()).toBe(false);
  });

  it("edits the details by hand", async () => {
    const calls = routes();
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await settle();
    await wrapper.get('[data-testid="edit-details"]').trigger("click");
    await settle();
    await typeInto($$('[data-testid="edit-title"]')[0] as HTMLInputElement, "Corrected Title");
    await typeInto($$('[data-testid="edit-narrator"]')[0] as HTMLInputElement, "A Better Narrator");
    $$('[data-testid="edit-save"]')[0].click();
    await settle();
    const patch = calls.find((c) => c.init?.method === "PATCH" && c.url.endsWith("/api/audiobooks/1"));
    expect(JSON.parse(String(patch?.init?.body))).toMatchObject({ title: "Corrected Title", narrator: "A Better Narrator" });
  });
});

describe("Audiobook controls", () => {
  /** Mount on the same pinia the book was started on (mountApp would make a fresh one). */
  function mountOnSamePinia(component: Component) {
    return mount(component, { attachTo: document.body, global: { plugins: [pinia] } });
  }
  let pinia = createPinia();

  async function playing() {
    pinia = createPinia();
    setActivePinia(pinia);
    mockFetch({
      "/api/audiobooks/1/position": () => json({ book_offset_ms: 0, updated_at: 1, finished: false }),
      "/api/audiobooks/1/settings": (_u: string, init?: RequestInit) => json({ speed: 1.25, skip_back_s: 15, skip_forward_s: 30, ...JSON.parse(String(init?.body)) }),
      "/api/audiobooks/1/bookmarks": () => json({ id: 3, book_id: 1, book_offset_ms: 5000, name: "Bookmark at 0:05", note: "", created_at: 1 }, 201),
      "/api/audiobooks/1": () => json(detail({ position_ms: 0 })),
      "/api/tracks/101": () => json(makeTrack({ id: 101, duration_ms: 36_000_000 })),
    });
    tauri.on("get_state", makeState({ status: "stopped", track: null }));
    const player = usePlayerStore();
    await player.init();
    const books = useAudiobooksStore();
    await books.start(1);
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ id: 101, duration_ms: 36_000_000 }), position_ms: 5000, duration_ms: 36_000_000, playback_rate: 1.25 }));
    await settle();
    return { books, player };
  }

  it("Now Playing shows the book, not the file: title, author and narrator, chapter, progress, speed and format", async () => {
    const { books } = await playing();
    const w = mountOnSamePinia(NowPlayingView);
    await settle();
    expect(w.get('[data-testid="np-kind"]').text()).toBe("Audiobook");
    expect(w.get('[data-testid="np-title"]').text()).toBe("Book 1");
    expect(w.get('[data-testid="np-byline"]').text()).toBe("by Ann Author · read by Nora Narrator");
    expect(w.get('[data-testid="np-series"]').text()).toBe("Saga, book 2");
    const chapter = w.get('[data-testid="np-chapter"]').text();
    expect(chapter).toContain("Chapter 1 of 2");
    expect(chapter).toContain("Opening");
    expect(chapter).toContain("0:05 of 16:40");
    expect(w.get('[data-testid="np-book-progress"]').text()).toContain("0:05 of 10 h");
    expect(w.get('[data-testid="np-speed"]').text()).toBe("1.25×");
    expect(w.get('[data-testid="np-book-format"]').text()).toMatch(/kHz/);
    expect(w.find('[data-placeholder="book"]').exists()).toBe(true);
    // Book actions, not the music ones.
    expect(w.find('[data-testid="add-to-queue"]').exists()).toBe(false);
    expect(w.find('[data-testid="np-back-to-music"]').exists()).toBe(!!books.stash);
    await w.get('[data-testid="np-book-details"]').trigger("click");
    expect(useNavStore().view).toEqual({ name: "audiobook", id: 1 });
    w.unmount();
  });

  it("skips by the book's own seconds, adds a bookmark, and changes speed", async () => {
    await playing();
    const wrapper = mountOnSamePinia(AudiobookControls);
    await settle();
    expect(wrapper.get('[data-testid="skip-back"]').text()).toContain("15");
    expect(wrapper.get('[data-testid="skip-forward"]').text()).toContain("30");
    await wrapper.get('[data-testid="skip-forward"]').trigger("click");
    await settle();
    // 5 s in, forward 30 s (the playhead interpolates with the wall clock, so allow a moment).
    const ms = (tauri.callsTo("seek_ms").at(-1) as { ms: number }).ms;
    expect(ms).toBeGreaterThanOrEqual(35_000);
    expect(ms).toBeLessThan(36_500);
    await wrapper.get('[data-testid="bookmark-here"]').trigger("click");
    await settle();
    expect(useAudiobooksStore().active?.bookmarks.length).toBe(2);
  });

  it("starts a sleep timer from its menu and shows the time left", async () => {
    const { books } = await playing();
    const wrapper = mountOnSamePinia(AudiobookControls);
    await settle();
    await openMenu(wrapper.get('[data-testid="sleep-button"]').element as HTMLElement);
    expect(menuItems().map((m) => m.textContent?.trim())).toEqual(expect.arrayContaining(["5 minutes", "60 minutes", "End of chapter"]));
    menuItems().find((m) => m.textContent?.includes("15 minutes"))!.click();
    await settle();
    expect(books.sleep).toMatchObject({ kind: "minutes", minutes: 15 });
    expect(wrapper.get('[data-testid="sleep-left"]').text()).toMatch(/^1[45]:\d\d$/);
    books.cancelSleep(true);
  });

  it("goes back to the music from the controls", async () => {
    const { books } = await playing();
    books.stash = { tracks: [makeTrack({ id: 1 })], index: 0, positionMs: 1000, repeat: "off", shuffle: false, wasPlaying: false };
    const wrapper = mountOnSamePinia(AudiobookControls);
    await settle();
    await wrapper.get('[data-testid="back-to-music"]').trigger("click");
    await settle();
    expect(tauri.callsTo("queue_restore")).toHaveLength(1);
    expect(books.active).toBeNull();
  });

  it("the seek bar spans the whole book while a book plays", async () => {
    await playing();
    const wrapper = mountOnSamePinia(SeekBar);
    await settle();
    expect(wrapper.get('[data-testid="duration"]').text()).toBe("10:00:00");
    expect(wrapper.get('[data-testid="elapsed"]').text()).toBe("0:05");
  });
});

describe("Audiobook listeners", () => {
  it("sends the chosen listener's name with audiobook calls only, and remembers the choice", async () => {
    const listeners = [
      { id: 0, name: "Default", books_started: 0 },
      { id: 1, name: "Ann B", books_started: 0 },
    ];
    const calls = mockFetch({
      "/api/audiobook-listeners": () => json(listeners),
      "/api/audiobooks": () => json([]),
    });
    mountApp(AudiobooksView);
    await settle();
    const before = calls.length; // the view's own first load, as Default
    await useAudiobooksStore().switchListener("Ann B");
    await settle();

    expect(localStorage.getItem("kahawai.audiobook-listener")).toBe("Ann B");
    const header = (c: { init?: RequestInit }) => (c.init?.headers as Record<string, string> | undefined)?.["X-Kahawai-Listener"];
    const library = calls.slice(before).filter((c) => c.url.includes("/api/audiobooks"));
    expect(library.length).toBeGreaterThan(0);
    expect(library.every((c) => header(c) === "Ann%20B")).toBe(true);
  });
});
