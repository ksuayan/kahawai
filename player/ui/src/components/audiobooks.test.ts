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
import type { Audiobook, AudiobookDetail } from "../types";
import AudiobookControls from "./AudiobookControls.vue";
import AudiobookDetailView from "./AudiobookDetail.vue";
import AudiobookSection from "./AudiobookSection.vue";
import AudiobooksView from "./AudiobooksView.vue";
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
  ...over,
});

const detail = (over: Partial<AudiobookDetail> = {}): AudiobookDetail => ({
  ...summary(1, { position_ms: 3_600_000, last_played_at: 5, progress: 0.1 }),
  narrator: "Nora Narrator",
  series: "Saga",
  series_index: 2,
  parts: [{ id: 1, track_id: 101, part_index: 0, title: "One", start_offset_ms: 0, duration_ms: 36_000_000 }],
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
    await cards[1].trigger("click");
    expect(useNavStore().view).toEqual({ name: "audiobook", id: 2 });
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
    expect(wrapper.get('[data-testid="bookmark"]').text()).toContain("Great line");
    expect(wrapper.get('[data-testid="history-day"]').text()).toContain("stopped at 1:00:00, 10 min listened");
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
    const play = tauri.callsTo("queue_play_at").at(-1) as { position_ms: number };
    expect(play.position_ms).toBe(3_600_000);
  });

  it("a chapter or a bookmark starts the book there", async () => {
    routes();
    tauri.on("get_state", makeState({ status: "stopped", track: null }));
    const { wrapper } = mountApp(AudiobookDetailView, { id: 1 });
    await usePlayerStore().init();
    await settle();
    await wrapper.findAll('[data-testid="chapter"]')[1].trigger("click");
    await settle();
    expect((tauri.callsTo("queue_play_at").at(-1) as { position_ms: number }).position_ms).toBe(1_000_000);
    await wrapper.get('[data-testid="bookmark"]').trigger("click");
    await settle();
    expect((tauri.callsTo("queue_play_at").at(-1) as { position_ms: number }).position_ms).toBe(120_000);
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

  it("skips by the book's own seconds, adds a bookmark, and changes speed", async () => {
    await playing();
    const wrapper = mountOnSamePinia(AudiobookControls);
    await settle();
    expect(wrapper.get('[data-testid="skip-back"]').text()).toContain("15");
    expect(wrapper.get('[data-testid="skip-forward"]').text()).toContain("30");
    await wrapper.get('[data-testid="skip-forward"]').trigger("click");
    await settle();
    expect(tauri.callsTo("seek_ms").at(-1)).toEqual({ ms: 35_000 });
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

describe("Settings: audiobook folders", () => {
  it("lists, adds and removes a folder, and shows the server's reason when it refuses", async () => {
    let roots = [{ id: 1, path: "/srv/books", name: "Books" }];
    const calls = mockFetch({
      "/api/audiobook-roots/1": (_u: string, init?: RequestInit) => {
        if (init?.method === "DELETE") roots = [];
        return new Response(null, { status: 204 });
      },
      "/api/audiobook-roots": (_u: string, init?: RequestInit) => {
        if (init?.method === "POST") {
          const body = JSON.parse(String(init.body));
          if (body.path === "/nope") return json({ error: "/nope is not a folder" }, 400);
          roots = [...roots, { id: 2, path: body.path, name: "More" }];
          return json(roots[1], 201);
        }
        return json(roots);
      },
      "/api/audiobooks": () => json([]),
    });
    const { wrapper } = mountApp(AudiobookSection);
    await settle();
    expect(wrapper.get('[data-testid="audiobook-roots"]').text()).toContain("/srv/books");
    await typeInto(wrapper.get<HTMLInputElement>('[data-testid="root-path"]').element, "/nope");
    await wrapper.get('[data-testid="add-root"]').trigger("click");
    await settle();
    expect(wrapper.text()).toContain("/nope is not a folder");
    await typeInto(wrapper.get<HTMLInputElement>('[data-testid="root-path"]').element, "/srv/more");
    await wrapper.get('[data-testid="add-root"]').trigger("click");
    await settle();
    expect(wrapper.findAll('[data-testid="remove-root"]')).toHaveLength(2);
    await wrapper.findAll('[data-testid="remove-root"]')[0].trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/audiobook-roots/1") && c.init?.method === "DELETE")).toBe(true);
  });
});

