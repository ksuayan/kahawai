import { beforeEach, describe, expect, it } from "vitest";
import { makeState, makeTrack, mockFetch } from "../test/fixtures";
import { $$, mountApp, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useRadioStore } from "../stores/radio";
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
