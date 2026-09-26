import { describe, expect, it } from "vitest";
import { makeTrack, mockFetch } from "../test/fixtures";
import { mountApp } from "../test/helpers";
import TrackRow from "./TrackRow.vue";

const mountRow = (props: Record<string, unknown>) => {
  mockFetch({ "/api/playlists": [] });
  return mountApp(TrackRow, props).wrapper;
};

describe("TrackRow", () => {
  it("shows number, title, artist — album, format badge and duration", () => {
    const w = mountRow({ track: makeTrack({ title: "Grenade", artist: "Bruno Mars", album: "Doo-Wops", track_no: 3, format: "mp3", bit_depth: null, sample_rate: 48000, duration_ms: 217_056 }) });
    expect(w.text()).toContain("3");
    expect(w.text()).toContain("Grenade");
    expect(w.text()).toContain("Bruno Mars — Doo-Wops");
    expect(w.text()).toContain("MP3 · 48kHz");
    expect(w.text()).toContain("3:37");
  });

  it("prefers an explicit number and falls back to a dash without a track number", () => {
    expect(mountRow({ track: makeTrack({ track_no: 9 }), number: 1 }).text()).toMatch(/^1/);
    expect(mountRow({ track: makeTrack({ track_no: null as never }) }).text()).toMatch(/^–/);
  });

  it("omits the artist line when there is neither artist nor album", () => {
    const w = mountRow({ track: makeTrack({ artist: null as never, album: null as never }) });
    expect(w.find(".text-xs").exists()).toBe(false);
  });

  it("highlights the current track (and does not highlight others)", () => {
    expect(mountRow({ track: makeTrack(), current: true }).attributes("data-current")).toBeDefined();
    expect(mountRow({ track: makeTrack() }).attributes("data-current")).toBeUndefined();
  });

  it("badges MQA tracks with the master rate, and only those", () => {
    const w = mountRow({ track: makeTrack({ format: "flac", mqa: true, original_sample_rate: 48000 }) });
    const badge = w.get('[data-testid="mqa-badge"]');
    expect(badge.text()).toBe("MQA · 48k");
    expect(badge.attributes("title")).toContain("plays as ordinary FLAC");
    expect(mountRow({ track: makeTrack({ mqa: false }) }).find('[data-testid="mqa-badge"]').exists()).toBe(false);
  });

  it("puts the full quality detail in the format badge tooltip", () => {
    const w = mountRow({ track: makeTrack({ format: "flac", bit_depth: 24, sample_rate: 96000, bitrate: 2400, channels: 2 }) });
    expect(w.get('[data-testid="format-badge"]').attributes("title")).toBe("FLAC · 24-bit / 96 kHz · 2400 kbps · 2 ch");
  });

  it("plays on double-click when playable", async () => {
    const track = makeTrack();
    const w = mountRow({ track });
    await w.trigger("dblclick");
    expect(w.emitted("play")).toEqual([[track]]);
  });

  it.each([
    ["missing", { missing: true }, /missing/i],
    ["not decodable", { decodable: false }, /extraction/i],
  ])("a %s track is dimmed, explains why, and does not play", async (_n, over, reason) => {
    const w = mountRow({ track: makeTrack({ title: "T", ...over } as never) });
    expect(w.attributes("data-playable")).toBe("false");
    expect(w.classes()).toContain("opacity-45");
    expect(w.attributes("title")).toMatch(reason);
    await w.trigger("dblclick");
    expect(w.emitted("play")).toBeUndefined();
  });

  it("shows the cover only when asked, and the action menu unless hidden", () => {
    expect(mountRow({ track: makeTrack() }).findComponent({ name: "Artwork" }).exists()).toBe(false);
    expect(mountRow({ track: makeTrack(), showArtwork: true }).findComponent({ name: "Artwork" }).exists()).toBe(true);
    expect(mountRow({ track: makeTrack(), showArtwork: true, artworkHash: "h" }).find("img").exists()).toBe(true);
    expect(mountRow({ track: makeTrack() }).find('button[aria-label^="Actions"]').exists()).toBe(true);
    expect(mountRow({ track: makeTrack(), showMenu: false }).find('button[aria-label^="Actions"]').exists()).toBe(false);
  });

  it("renders extra controls passed in the slot", () => {
    const { wrapper } = mountApp(TrackRow, { track: makeTrack() }, { default: '<button id="extra">↑</button>' });
    expect(wrapper.find("#extra").exists()).toBe(true);
  });
});
