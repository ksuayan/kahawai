import { beforeEach, describe, expect, it } from "vitest";
import { makeState, makeTrack, mockFetch } from "../test/fixtures";
import { $$, mountApp, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useRadioStore } from "../stores/radio";
import { useViewPrefsStore } from "../stores/viewPrefs";
import { usePlayerStore } from "../stores/player";
import NowPlayingBar from "./NowPlayingBar.vue";
import RadioView from "./RadioView.vue";
import SeekBar from "./SeekBar.vue";

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

const fav = (id: number, name: string, over: Record<string, unknown> = {}) => ({
  id,
  station_uuid: null,
  name,
  url: `http://s${id}/live`,
  url_resolved: null,
  homepage: null,
  favicon: null,
  tags: null,
  country: "Fiji",
  language: null,
  bitrate: 96,
  codec: "MP3",
  manual: true,
  sort_order: id,
  added_at: 0,
  ...over,
});

beforeEach(() => {
  tauri.reset();
  tauri.on("get_state", makeState({ status: "stopped", track: null }));
});

describe("Radio view", () => {
  it("lists favorites with codec badges, plays one, and removes one", async () => {
    let list = [fav(1, "Island FM"), fav(2, "Reef Radio")];
    const calls = mockFetch({
      "/api/radio/favorites/1/play": () => json({ name: "Island FM", url: "http://s1/live", codec: "MP3", bitrate: 96, needs_relay: false }),
      "/api/radio/favorites/2": () => {
        list = list.filter((f) => f.id !== 2);
        return new Response(null, { status: 204 });
      },
      "/api/radio/favorites": () => json(list),
    });
    const { wrapper } = mountApp(RadioView);
    await settle();
    const rows = wrapper.findAll('[data-testid="station-row"]');
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("Island FM");
    expect(rows[0].text()).toContain("MP3 · 96 kbps");
    await rows[0].get('[data-testid="station-play"]').trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/favorites/1/play"))).toBe(true);
    expect((tauri.callsTo("queue_play").at(-1) as { tracks: { id: number }[] }).tracks[0].id).toBe(-1);
    await wrapper.findAll('[data-testid="remove-favorite"]')[1].trigger("click");
    await settle();
    expect(wrapper.findAll('[data-testid="station-row"]')).toHaveLength(1);
  });

  it("says when there are none, and adds one by address or shows why it cannot", async () => {
    let list: ReturnType<typeof fav>[] = [];
    mockFetch({
      "/api/radio/favorites": (_u: string, init?: RequestInit) => {
        if (init?.method !== "POST") return json(list);
        const url = JSON.parse(String(init.body)).url as string;
        if (url.endsWith(".pls")) return json({ error: "that address is a playlist (.pls / .m3u); paste the stream address inside it" }, 400);
        list = [fav(5, "Found FM")];
        return json(list[0], 201);
      },
    });
    const { wrapper } = mountApp(RadioView);
    await settle();
    expect(wrapper.text()).toContain("No favorite stations yet");
    await typeInto(wrapper.get<HTMLInputElement>('[data-testid="station-address"]').element, "http://x/list.pls");
    await wrapper.get('[data-testid="add-station"]').trigger("click");
    await settle();
    expect(wrapper.text()).toContain("that address is a playlist");
    await typeInto(wrapper.get<HTMLInputElement>('[data-testid="station-address"]').element, "http://x/live.mp3");
    await wrapper.get('[data-testid="add-station"]').trigger("click");
    await settle();
    expect(wrapper.text()).toContain("Found FM");
  });

  it("Find stations explains when the server's directory is off", async () => {
    mockFetch({
      "/api/radio/favorites": () => json([]),
      "/api/radio/facets/tags": () => json({ error: "the online station directory is off: turn on Online sources" }, 400),
      "/api/radio/facets/countries": () => json({ error: "the online station directory is off: turn on Online sources" }, 400),
      "/api/radio/facets/languages": () => json({ error: "the online station directory is off: turn on Online sources" }, 400),
      "/api/radio/search": () => json({ error: "the online station directory is off: turn on Online sources" }, 400),
    });
    const { wrapper } = mountApp(RadioView);
    await settle();
    await wrapper.get('[data-testid="tab-browse"]').trigger("click");
    await settle();
    expect(wrapper.get('[data-testid="directory-off"]').text()).toContain("Online sources");
  });

  it("Find stations lists results, saves one as a favorite, and cannot play an HLS station", async () => {
    let saved = false;
    mockFetch({
      "/api/radio/favorites": (_u: string, init?: RequestInit) => {
        if (init?.method === "POST") saved = true;
        return json(saved ? [fav(7, "Rock One", { station_uuid: "u1", manual: false })] : []);
      },
      "/api/radio/facets/tags": () => json([{ name: "rock", stations: 5 }]),
      "/api/radio/facets/countries": () => json([]),
      "/api/radio/facets/languages": () => json([]),
      "/api/radio/search": () =>
        json([
          { station_uuid: "u1", name: "Rock One", url: "http://r1/live", url_resolved: null, homepage: null, favicon: null, tags: "rock,classic", country: "UK", language: null, codec: "MP3", bitrate: 128, clicks: 1200, hls: false, needs_relay: false },
          { station_uuid: "u2", name: "Stream HLS", url: "http://r2/x.m3u8", url_resolved: null, homepage: null, favicon: null, tags: null, country: null, language: null, codec: "AAC", bitrate: 64, clicks: 3, hls: true, needs_relay: false },
        ]),
    });
    const { wrapper } = mountApp(RadioView);
    await settle();
    await wrapper.get('[data-testid="tab-browse"]').trigger("click");
    await settle();
    const rows = wrapper.findAll('[data-testid="results"] [data-testid="station-row"]');
    expect(rows).toHaveLength(2);
    expect(rows[0].text()).toContain("1,200 listens");
    expect(rows[1].get('[data-testid="station-unplayable"]').text()).toContain("HLS");
    expect((rows[1].get('[data-testid="station-play"]').element as HTMLButtonElement).disabled).toBe(true);
    await rows[0].get('[data-testid="favorite-station"]').trigger("click");
    await settle();
    expect(saved).toBe(true);
    expect((wrapper.findAll('[data-testid="favorite-station"]')[0].element as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("Radio view: layout and order", () => {
  const three = () => [fav(1, "Zulu Radio", { bitrate: 64, codec: "AAC" }), fav(2, "Alpha FM", { bitrate: 128, codec: "MP3" }), fav(3, "Mike Live", { bitrate: 320, codec: "AAC" })];
  const names = (w: ReturnType<typeof mountApp>["wrapper"]) => w.findAll('[data-testid="station-row"]').map((r) => r.find(".font-semibold").text());

  it("puts the stream address bar first, under the heading, on both tabs", async () => {
    mockFetch({ "/api/radio/favorites": () => json(three()), "/api/radio/facets": () => json([]), "/api/radio/search": () => json([]) });
    const { wrapper } = mountApp(RadioView);
    await settle();
    const html = wrapper.html();
    expect(html.indexOf('data-testid="add-by-address"')).toBeLessThan(html.indexOf('data-testid="favorites"'));
    await wrapper.get('[data-testid="tab-browse"]').trigger("click");
    await settle();
    expect(wrapper.find('[data-testid="station-address"]').exists()).toBe(true);
  });

  it("shows stations as a grid of tiles, and remembers it", async () => {
    const calls = mockFetch({
      "/api/radio/favorites/1/play": () => json({ name: "Zulu Radio", url: "http://s1/live", codec: "AAC", bitrate: 64, needs_relay: false }),
      "/api/radio/favorites": () => json(three()),
    });
    const { wrapper } = mountApp(RadioView);
    await settle();
    expect(wrapper.get('[data-testid="station-row"]').attributes("data-layout")).toBeUndefined();
    await wrapper.get('button[aria-label="Grid"]').trigger("click");
    await settle();
    expect(useViewPrefsStore().prefs.radioLayout).toBe("grid");
    const tiles = wrapper.findAll('[data-testid="station-row"]');
    expect(tiles.every((t) => t.attributes("data-layout") === "grid")).toBe(true);
    await tiles[0].get('[data-testid="station-tile-play"]').trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/favorites/1/play"))).toBe(true);
    expect((tauri.callsTo("queue_play").at(-1) as { tracks: { id: number }[] }).tracks[0].id).toBe(-1);
  });

  it("sorts by name, bandwidth or format, and only lets you reorder favorites as listed", async () => {
    mockFetch({ "/api/radio/favorites": () => json(three()) });
    const { wrapper } = mountApp(RadioView);
    await settle();
    expect(wrapper.text()).toContain("Sort by");
    expect(names(wrapper)).toEqual(["Zulu Radio", "Alpha FM", "Mike Live"]);
    expect(wrapper.findAll('[data-testid="move-down"]')[0].attributes("disabled")).toBeUndefined();
    const prefs = useViewPrefsStore().prefs;
    prefs.radioSort = "name";
    await settle();
    expect(names(wrapper)).toEqual(["Alpha FM", "Mike Live", "Zulu Radio"]);
    expect(wrapper.findAll('[data-testid="move-down"]').every((b) => b.attributes("disabled") !== undefined)).toBe(true);
    prefs.radioSort = "bandwidth";
    await settle();
    expect(names(wrapper)).toEqual(["Mike Live", "Alpha FM", "Zulu Radio"]);
    prefs.radioSort = "format";
    await settle();
    expect(names(wrapper)).toEqual(["Mike Live", "Zulu Radio", "Alpha FM"]);
  });
});

describe("Now playing a station", () => {
  const play = async (radio: Record<string, unknown>) => {
    const track = makeTrack({ id: -4, title: "Island FM", artist: "Live radio", duration_ms: null, album_id: null });
    tauri.emit("player-state", makeState({ status: "playing", track, position_ms: 90_000, radio } as never));
    await settle();
  };

  it("the bar shows the station, the live song title, and no timeline", async () => {
    mockFetch({});
    const { wrapper, pinia } = mountApp(NowPlayingBar);
    await usePlayerStore(pinia).init();
    await play({ title: "Artist - Song", reconnecting: false, attempt: 0, bitrate_kbps: 96, reason: null });
    expect(wrapper.get('[data-testid="title"]').text()).toBe("Island FM");
    expect(wrapper.get('[data-testid="artist"]').text()).toBe("Artist - Song");
    expect(wrapper.get('[data-testid="elapsed"]').text()).toBe("LIVE");
    expect(wrapper.get('[data-testid="duration"]').text()).toBe("");
    expect(wrapper.find('[data-testid="radio-reconnecting"]').exists()).toBe(false);
    expect(useRadioStore(pinia).isPlaying).toBe(true);
  });

  it("the bar's cover is the sidebar's radio icon, not a music note", async () => {
    mockFetch({});
    const { wrapper, pinia } = mountApp(NowPlayingBar);
    await usePlayerStore(pinia).init();
    await play({ title: "Artist - Song", reconnecting: false, attempt: 0, bitrate_kbps: 96, reason: null });
    const cover = wrapper.get('[data-placeholder]');
    expect(cover.attributes("data-placeholder")).toBe("radio");
    expect(cover.find("svg").classes().join(" ")).toContain("lucide-radio");
    // Back to a music track: the note again.
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ id: 5, album_id: null }) }));
    await settle();
    expect(wrapper.get('[data-placeholder]').attributes("data-placeholder")).toBe("music");
  });

  it("says Live radio when the station sends no titles, and shows a lost connection", async () => {
    mockFetch({});
    const { wrapper, pinia } = mountApp(NowPlayingBar);
    await usePlayerStore(pinia).init();
    await play({ title: null, reconnecting: true, attempt: 2, bitrate_kbps: null, reason: "x" });
    expect(wrapper.get('[data-testid="artist"]').text()).toBe("Live radio");
    expect(wrapper.get('[data-testid="radio-reconnecting"]').text()).toContain("Trying again");
    expect(wrapper.get('[data-testid="radio-reconnecting"]').text()).toContain("try 2");
  });

  it("the seek bar is disabled while a station plays", async () => {
    mockFetch({});
    const { wrapper, pinia } = mountApp(SeekBar);
    await usePlayerStore(pinia).init();
    await play({ title: null, reconnecting: false, attempt: 0, bitrate_kbps: null, reason: null });
    expect(wrapper.get('[data-testid="elapsed"]').text()).toBe("LIVE");
    expect($$('[role="slider"]').every((s) => s.getAttribute("data-disabled") !== null || s.getAttribute("aria-disabled") === "true")).toBe(true);
  });
});
