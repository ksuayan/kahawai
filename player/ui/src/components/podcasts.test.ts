import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia, type Pinia } from "pinia";
import type { Component } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { episodeTrackId } from "../lib/podcast";
import { makeEpisode, makeState, makeTrack, mockFetch } from "../test/fixtures";
import { $$, bodyOf, dialog, menuItems, mountApp, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useJobsStore } from "../stores/jobs";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import { usePodcastsStore } from "../stores/podcasts";
import { useViewPrefsStore } from "../stores/viewPrefs";
import type { PodcastEpisode, PodcastFeed } from "../types";
import NowPlayingBar from "./NowPlayingBar.vue";
import NowPlayingView from "./NowPlayingView.vue";
import PodcastDownloadsView from "./PodcastDownloadsView.vue";
import PodcastEpisodeView from "./PodcastEpisodeView.vue";
import PodcastFeedView from "./PodcastFeedView.vue";
import PodcastsView from "./PodcastsView.vue";

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

function feed(over: Partial<PodcastFeed> = {}): PodcastFeed {
  return {
    id: 3,
    feed_url: "https://show.example/feed",
    title: "The Show",
    author: "Host",
    description: "A show about things.",
    link: "https://show.example",
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
    episode_count: 2,
    unplayed_count: 2,
    speed: 1.25,
    skip_back_s: 15,
    skip_forward_s: 30,
    auto_advance: false,
    ...over,
  };
}

const e1 = makeEpisode({ id: 11, feed_id: 3, title: "One", published_at: Date.UTC(2026, 1, 1) });
const e2 = makeEpisode({ id: 12, feed_id: 3, title: "Two", published_at: Date.UTC(2026, 1, 8), downloaded: true, file_bytes: 12_000_000 });

beforeEach(() => {
  localStorage.clear();
  tauri.reset();
});

describe("Podcasts", () => {
  function routes(feeds: PodcastFeed[] = [feed(), feed({ id: 4, title: "Failing", unplayed_count: 0, last_error: "HTTP 404" })], inProgress: PodcastEpisode[] = []) {
    return mockFetch({
      "/api/podcasts/feeds/import-opml": () => json({ added: 2, already_subscribed: 1, invalid: [] }),
      "/api/podcasts/feeds": (_u: string, init?: RequestInit) =>
        init?.method === "POST" ? json({ feed: feed({ id: 9, title: "New Show" }), episodes_added: 4, warnings: [] }, 201) : json(feeds),
      "/api/podcasts/in-progress": () => json(inProgress),
    });
  }

  it("shows each show with its unplayed count and marks a feed that can't be read", async () => {
    routes();
    const { wrapper } = mountApp(PodcastsView);
    await settle();
    const cards = wrapper.findAll('[data-testid="podcast-card"]');
    expect(cards.map((c) => c.text())).toEqual([expect.stringContaining("The Show"), expect.stringContaining("Failing")]);
    expect(cards[0].get('[data-testid="podcast-unplayed"]').text()).toBe("2");
    expect(cards[1].find('[data-testid="podcast-unplayed"]').exists()).toBe(false);
    expect(cards[1].get('[data-testid="podcast-failing"]').attributes("title")).toContain("HTTP 404");
    await cards[0].trigger("click");
    expect(useNavStore().view).toEqual({ name: "podcast", id: 3 });
  });

  it("switches to a list", async () => {
    routes();
    const { wrapper } = mountApp(PodcastsView, {}, {}, () => (useViewPrefsStore().prefs.podcastsLayout = "list"));
    await settle();
    const rows = wrapper.findAll('[data-testid="podcast-row"]');
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("2 unplayed");
    expect(rows[1].text()).toContain("Can't be read");
  });

  it("adds a show by its feed address and opens it", async () => {
    const calls = routes();
    const { wrapper } = mountApp(PodcastsView);
    await settle();
    await wrapper.get('[data-testid="podcast-add"]').trigger("click");
    await settle();
    expect(dialog()!.textContent).toContain("Add a podcast");
    await typeInto(dialog()!.querySelector("input")!, "https://new.example/feed");
    $$("[role=dialog] button").find((b) => b.textContent?.trim() === "Subscribe")!.click();
    await settle();
    const post = calls.find((c) => c.init?.method === "POST" && c.url.endsWith("/api/podcasts/feeds"));
    expect(bodyOf(post?.init)).toEqual({ url: "https://new.example/feed" });
    expect(useNavStore().view).toEqual({ name: "podcast", id: 9 });
  });

  it("imports an OPML file from another app", async () => {
    const calls = routes();
    const { wrapper } = mountApp(PodcastsView);
    await settle();
    const input = wrapper.get('[data-testid="podcast-import-file"]').element as HTMLInputElement;
    const file = new File(["<opml><body><outline xmlUrl=\"https://a.example/f\"/></body></opml>"], "subs.opml", { type: "text/xml" });
    Object.defineProperty(input, "files", { value: [file] });
    input.dispatchEvent(new Event("change"));
    await settle();
    await settle();
    const post = calls.find((c) => c.url.endsWith("/import-opml"));
    expect(String(post?.init?.body)).toContain("https://a.example/f");
    const { useToastsStore } = await import("../stores/toasts");
    expect(useToastsStore().toasts.at(-1)?.detail).toContain("2 added · 1 already subscribed");
  });

  it("lists Up Next and the episodes you are part-way through", async () => {
    routes([feed()], [makeEpisode({ id: 20, title: "Half done", position_ms: 300_000 })]);
    const { wrapper } = mountApp(PodcastsView, {}, {}, () => (usePodcastsStore().upNext = [e1, e2]));
    await settle();
    expect(wrapper.findAll('[data-testid="up-next-row"]').map((r) => r.text())).toEqual([expect.stringContaining("One"), expect.stringContaining("Two")]);
    expect(wrapper.get('[data-testid="podcast-continue"]').text()).toContain("Half done");
    expect(wrapper.get('[data-testid="podcast-continue"]').text()).toContain("5:00 left");
    await wrapper.findAll('[data-testid="up-next-remove"]')[0].trigger("click");
    await settle();
    expect(usePodcastsStore().upNext.map((e) => e.id)).toEqual([12]);
  });

  it("explains how to start when there are none", async () => {
    routes([]);
    const { wrapper } = mountApp(PodcastsView);
    await settle();
    expect(wrapper.text()).toContain("Add one by its feed address, or import an OPML file");
  });
});

describe("Right-click menus", () => {
  const rightClick = async (el: Element) => {
    el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
    await settle();
  };
  const item = (label: string) => menuItems().find((m) => m.textContent?.trim() === label)!;

  function routes() {
    return mockFetch({
      "/api/podcasts/feeds/3/episodes": (url: string) => json(url.includes("unplayed=1") ? [e2, e1] : [e2, e1]),
      "/api/podcasts/feeds/3/settings": (_u: string, init?: RequestInit) => json({ ...feed(), ...JSON.parse(String(init?.body)) }),
      "/api/podcasts/feeds/3": () => new Response(null, { status: 204 }),
      "/api/podcasts/feeds": () => json([feed()]),
      "/api/podcasts/in-progress": () => json([]),
      "/api/podcasts/refresh": () => json([{ feed_id: 3, episodes_added: 0, error: null }]),
      "/api/podcasts/episodes/12/download": () => new Response(null, { status: 204 }),
      "/api/podcasts/episodes/11/played": () => new Response(null, { status: 204 }),
      "/api/podcasts/episodes/12/position": () => json({ offset_ms: 0, updated_at: 1, played: false }),
      "/api/jobs": () => json([]),
    });
  }

  it("a show's menu plays its newest unplayed episode, checks for new, opens its settings and website, and deletes it after asking", async () => {
    const calls = routes();
    tauri.on("get_state", makeState({ status: "stopped", track: null }));
    const { wrapper } = mountApp(PodcastsView);
    await usePlayerStore().init();
    await settle();
    const card = wrapper.get('[data-testid="podcast-card"]').element;
    await rightClick(card);
    expect(menuItems().map((m) => m.textContent?.trim())).toEqual(["Play", "Check for new", "Settings", "Website", "Delete"]);

    item("Play").click();
    await settle();
    const play = tauri.callsTo("queue_play_at").at(-1) as { tracks: { id: number }[] };
    expect(play.tracks[0].id).toBe(episodeTrackId(12)); // the newest unplayed

    await rightClick(card);
    item("Check for new").click();
    await settle();
    expect(bodyOf(calls.find((c) => c.url.endsWith("/api/podcasts/refresh"))?.init)).toEqual({ feed_id: 3 });

    await rightClick(card);
    item("Settings").click();
    await settle();
    expect(dialog()!.textContent).toContain("The Show: settings");
    $$("[role=dialog] button").find((b) => b.textContent?.trim() === "Cancel")!.click();
    await settle();

    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    await rightClick(card);
    item("Website").click();
    await settle();
    expect(open).toHaveBeenCalledWith("https://show.example", "_blank", "noopener");
    open.mockRestore();

    await rightClick(card);
    item("Delete").click();
    await settle();
    expect(dialog()!.textContent).toContain("Delete The Show?");
    $$("[role=dialog] button").find((b) => b.textContent?.trim() === "Delete")!.click();
    await settle();
    expect(calls.some((c) => c.url.endsWith("/api/podcasts/feeds/3") && c.init?.method === "DELETE")).toBe(true);
  });

  it("an episode's menu plays it, adds it to Up Next, marks it played, and deletes its download", async () => {
    const calls = routes();
    const { wrapper } = mountApp(PodcastFeedView, { id: 3 });
    await settle();
    const [two, one] = wrapper.findAll('[data-testid="episode-row"]');
    await rightClick(one.element);
    expect(menuItems().map((m) => m.textContent?.trim())).toEqual(["Play", "Add to Up Next", "Mark played", "Delete download"]);
    expect(item("Delete download").hasAttribute("data-disabled")).toBe(true); // nothing downloaded

    item("Add to Up Next").click();
    await settle();
    expect(usePodcastsStore().upNext.map((e) => e.id)).toEqual([11]);

    await rightClick(one.element);
    expect(item("Remove from Up Next")).toBeTruthy();
    item("Mark played").click();
    await settle();
    expect(bodyOf(calls.find((c) => c.url.endsWith("/episodes/11/played"))?.init)).toEqual({ played: true });

    await rightClick(two.element);
    item("Delete download").click();
    await settle();
    expect(calls.some((c) => c.url.endsWith("/episodes/12/download") && c.init?.method === "DELETE")).toBe(true);
  });
});

describe("A show", () => {
  function routes(f = feed()) {
    return mockFetch({
      "/api/podcasts/feeds/3/episodes": (url: string) => json(url.includes("unplayed=1") ? [e1] : [e2, e1]),
      "/api/podcasts/feeds/3/settings": (_u: string, init?: RequestInit) => json({ ...f, ...JSON.parse(String(init?.body)) }),
      "/api/podcasts/feeds/3": () => new Response(null, { status: 204 }),
      "/api/podcasts/feeds": () => json([f]),
      "/api/podcasts/in-progress": () => json([]),
      "/api/podcasts/episodes/11/download": () => json({ id: "job-1", kind: "podcast_download", label: "Download: One", status: "queued", progress: 0, payload: "11" }, 202),
      "/api/podcasts/episodes/12/download": () => new Response(null, { status: 204 }),
      "/api/podcasts/episodes/11/played": () => new Response(null, { status: 204 }),
      "/api/podcasts/refresh": () => json([{ feed_id: 3, episodes_added: 1, error: null }]),
      "/api/jobs": () => json([]),
    });
  }

  it("shows the show and its episodes newest first, with their download state", async () => {
    routes(feed({ last_error: "timed out" }));
    const { wrapper } = mountApp(PodcastFeedView, { id: 3 });
    await settle();
    expect(wrapper.get('[data-testid="podcast-title"]').text()).toBe("The Show");
    expect(wrapper.get('[data-testid="podcast-error"]').text()).toContain("timed out");
    const rows = wrapper.findAll('[data-testid="episode-row"]');
    expect(rows.map((r) => r.get('[data-testid="episode-title"]').text())).toEqual(["Two", "One"]);
    expect(rows[0].get('[data-testid="episode-download-state"]').text()).toBe("Downloaded · 11.4 MB");
    expect(rows[1].get('[data-testid="episode-download-state"]').text()).toBe("Streams");
  });

  it("filters to unplayed episodes", async () => {
    const calls = routes();
    const { wrapper } = mountApp(PodcastFeedView, { id: 3 });
    await settle();
    usePodcastsStore().unplayedOnly = true;
    await settle();
    expect(calls.some((c) => c.url.includes("/feeds/3/episodes?unplayed=1"))).toBe(true);
    expect(wrapper.findAll('[data-testid="episode-row"]')).toHaveLength(1);
  });

  it("downloads an episode, shows its progress, deletes a download and marks played", async () => {
    const calls = routes();
    const { wrapper } = mountApp(PodcastFeedView, { id: 3 });
    await settle();
    const one = () => wrapper.findAll('[data-testid="episode-row"]')[1];
    await one().get('[data-testid="episode-download"]').trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/episodes/11/download") && c.init?.method === "POST")).toBe(true);
    useJobsStore().jobs = [{ id: "job-1", kind: "podcast_download", label: "Download: One", status: "running", progress: 0.35, payload: "11" } as never];
    await settle();
    expect(one().get('[data-testid="episode-download-state"]').text()).toContain("Downloading 35%");
    await wrapper.findAll('[data-testid="episode-row"]')[0].get('[data-testid="episode-delete"]').trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/episodes/12/download") && c.init?.method === "DELETE")).toBe(true);
    await one().get('[data-testid="episode-mark"]').trigger("click");
    await settle();
    const played = calls.find((c) => c.url.endsWith("/episodes/11/played"));
    expect(bodyOf(played?.init)).toEqual({ played: true });
  });

  it("saves the show's settings", async () => {
    const calls = routes();
    const { wrapper } = mountApp(PodcastFeedView, { id: 3 });
    await settle();
    await wrapper.get('[data-testid="podcast-settings-open"]').trigger("click");
    await settle();
    const skip = dialog()!.querySelector('input[aria-label="Skip forward seconds"]') as HTMLInputElement;
    await typeInto(skip, "60");
    (dialog()!.querySelector('[data-testid="podcast-settings-save"]') as HTMLElement).click();
    await settle();
    const put = calls.filter((c) => c.url.endsWith("/feeds/3/settings")).at(-1);
    expect(bodyOf(put?.init)).toMatchObject({ skip_forward_s: 60, speed: 1.25, keep_n: 5, auto_advance: false });
    expect(dialog()).toBeNull();
  });

  it("unsubscribes after asking, and goes back to Podcasts", async () => {
    const calls = routes();
    const { wrapper } = mountApp(PodcastFeedView, { id: 3 });
    await settle();
    await wrapper.get('[data-testid="podcast-unsubscribe"]').trigger("click");
    await settle();
    $$("[role=dialog] button").find((b) => b.textContent?.trim() === "Unsubscribe")!.click();
    await settle();
    expect(calls.some((c) => c.url.endsWith("/api/podcasts/feeds/3") && c.init?.method === "DELETE")).toBe(true);
    expect(useNavStore().view.name).toBe("podcasts");
  });
});

describe("An episode", () => {
  const detail = {
    ...makeEpisode({ id: 11, feed_id: 3, title: "One", position_ms: 150_000, link: "https://show.example/1", season: 2, episode: 5 }),
    description_html: '<p>Notes with <a href="https://ref.example/x">a link</a>.</p><script>bad()</script>',
    feed: feed(),
  };

  function routes() {
    return mockFetch({
      "/api/podcasts/episodes/11/history": () => json([{ id: 1, started_at: Date.UTC(2026, 2, 1, 9), ended_at: Date.UTC(2026, 2, 1, 9, 30), start_offset_ms: 0, end_offset_ms: 150_000, listened_ms: 150_000 }]),
      "/api/podcasts/episodes/11": () => json(detail),
      "/api/podcasts/feeds": () => json([feed()]),
    });
  }

  it("shows the notes made safe, where you are, and when you listened", async () => {
    routes();
    const { wrapper } = mountApp(PodcastEpisodeView, { id: 11 });
    await settle();
    expect(wrapper.get('[data-testid="episode-heading"]').text()).toBe("One");
    expect(wrapper.get('[data-testid="episode-facts"]').text()).toContain("Season 2, Episode 5");
    expect(wrapper.get('[data-testid="episode-detail-play"]').text()).toContain("Continue from 2:30");
    const notes = wrapper.get('[data-testid="show-notes"]');
    expect(notes.html()).not.toContain("script");
    expect(notes.text()).toContain("Notes with a link.");
    expect(wrapper.findAll('[data-testid="episode-history-day"]')).toHaveLength(1);
  });

  it("opens a show-notes link in the browser, not in the app", async () => {
    routes();
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    const { wrapper } = mountApp(PodcastEpisodeView, { id: 11 });
    await settle();
    await wrapper.get('[data-testid="show-notes"] a').trigger("click");
    expect(open).toHaveBeenCalledWith("https://ref.example/x", "_blank", "noopener");
    open.mockRestore();
  });

  it("goes to its show", async () => {
    routes();
    const { wrapper } = mountApp(PodcastEpisodeView, { id: 11 });
    await settle();
    await wrapper.get('[data-testid="episode-show"]').trigger("click");
    expect(useNavStore().view).toEqual({ name: "podcast", id: 3 });
  });
});

describe("Downloads", () => {
  it("lists what the server holds with sizes, deletes a file, and edits each show's rules", async () => {
    const calls = mockFetch({
      "/api/podcasts/downloads": () => json([{ ...e2, feed_title: "The Show" }]),
      "/api/podcasts/folder": () => json({ path: "/srv/podcasts", custom: false, usable: true, episodes_downloaded: 1, bytes_downloaded: 12_000_000 }),
      "/api/podcasts/feeds/3/settings": (_u: string, init?: RequestInit) => json({ ...feed(), ...JSON.parse(String(init?.body)) }),
      "/api/podcasts/feeds": () => json([feed()]),
      "/api/podcasts/in-progress": () => json([]),
      "/api/podcasts/episodes/12/download": () => new Response(null, { status: 204 }),
      "/api/jobs": () => json([]),
    });
    const { wrapper } = mountApp(PodcastDownloadsView);
    await settle();
    expect(wrapper.get('[data-testid="downloads-folder"]').text()).toContain("1 episodes, 11.4 MB");
    expect(wrapper.get('[data-testid="download-size"]').text()).toBe("11.4 MB");
    const rule = wrapper.get('[data-testid="download-rule"]');
    await rule.get('[role="switch"]').trigger("click");
    await settle();
    expect(bodyOf(calls.filter((c) => c.url.endsWith("/feeds/3/settings")).at(-1)?.init)).toEqual({ auto_download: false });
    await wrapper.get('[data-testid="download-delete"]').trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/episodes/12/download") && c.init?.method === "DELETE")).toBe(true);
    expect(wrapper.findAll('[data-testid="download-row"]')).toHaveLength(0);
  });
});

describe("Playing an episode", () => {
  let pinia: Pinia;
  const mountOnSamePinia = (component: Component) => mount(component, { attachTo: document.body, global: { plugins: [pinia] } });

  async function playing() {
    pinia = createPinia();
    setActivePinia(pinia);
    mockFetch({
      "/api/podcasts/episodes/11/position": () => json({ offset_ms: 0, updated_at: 1, played: false }),
      "/api/podcasts/episodes/11": () => json({ ...e1, feed: feed() }),
      "/api/podcasts/feeds": () => json([feed()]),
      "/api/podcasts/in-progress": () => json([]),
    });
    tauri.on("get_state", makeState({ status: "stopped", track: null }));
    await usePlayerStore().init();
    const podcasts = usePodcastsStore();
    await podcasts.play(e1);
    const track = { ...makeTrack({ id: episodeTrackId(11), duration_ms: 600_000, format: "mp3", sample_rate: 44100, bitrate: 128, channels: 2 }), title: "One" };
    tauri.emit("player-state", makeState({ status: "playing", track, position_ms: 60_000, duration_ms: 600_000 }));
    await settle();
    return podcasts;
  }

  it("the bar shows the episode and its show, the podcast controls, and Pause on its Play buttons", async () => {
    const podcasts = await playing();
    const w = mountOnSamePinia(NowPlayingBar);
    await settle();
    expect(w.get('[data-testid="title"]').text()).toBe("One");
    expect(w.get('[data-testid="artist"]').text()).toBe("The Show");
    expect(w.get('[data-placeholder]').attributes("data-placeholder")).toBe("podcast");
    expect(w.find('[data-testid="podcast-controls"]').exists()).toBe(true);
    await w.get('[data-testid="podcast-skip-forward"]').trigger("click");
    // 60 s in, plus the show's 30 s (the playhead keeps moving between the event and the click).
    const ms = (tauri.callsTo("seek_ms").at(-1) as { ms: number }).ms;
    expect(ms).toBeGreaterThanOrEqual(90_000);
    expect(ms).toBeLessThan(91_000);
    expect(podcasts.isActive).toBe(true);
    w.unmount();
  });

  it("Now Playing shows the episode, not the file", async () => {
    await playing();
    const w = mountOnSamePinia(NowPlayingView);
    await settle();
    expect(w.get('[data-testid="np-kind"]').text()).toBe("Podcast");
    expect(w.get('[data-testid="np-title"]').text()).toBe("One");
    expect(w.get('[data-testid="np-show"]').text()).toBe("The Show");
    expect(w.get('[data-testid="np-speed"]').text()).toBe("1.25×");
    expect(w.get('[data-testid="np-episode-source"]').text()).toBe("Streaming");
    expect(w.get('[data-testid="np-episode-progress"]').text()).toContain("1:00 of 10 min");
    expect(w.find('[data-testid="add-to-queue"]').exists()).toBe(false);
    await w.get('[data-testid="np-episode-details"]').trigger("click");
    expect(useNavStore().view).toEqual({ name: "episode", id: 11 });
    w.unmount();
  });

  it("an episode row's Play becomes Pause while it plays", async () => {
    await playing();
    mockFetch({
      "/api/podcasts/feeds/3/episodes": () => json([e1]),
      "/api/podcasts/feeds": () => json([feed()]),
      "/api/podcasts/in-progress": () => json([]),
      "/api/podcasts/episodes/11/position": () => json({ offset_ms: 0, updated_at: 1, played: false }),
    });
    const w = mount(PodcastFeedView, { props: { id: 3 }, attachTo: document.body, global: { plugins: [pinia] } });
    await settle();
    expect(w.get('[data-testid="episode-play"]').attributes("title")).toBe("Pause");
    expect(w.get('[data-testid="podcast-play"]').text()).toContain("Pause");
    await w.get('[data-testid="episode-play"]').trigger("click");
    await settle();
    expect(tauri.callsTo("pause")).toHaveLength(1);
    w.unmount();
  });
});
