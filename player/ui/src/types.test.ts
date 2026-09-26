import { describe, expect, it } from "vitest";
import { makeTrack } from "./test/fixtures";
import {
  formatBadge,
  formatDuration,
  isPlayable,
  trackTitle,
  unplayableReason,
  validFormatsFor,
} from "./types";

describe("formatDuration", () => {
  it.each([
    [0, "0:00"],
    [999, "0:00"],
    [1000, "0:01"],
    [61_000, "1:01"],
    [3_599_000, "59:59"],
    [3_600_000, "1:00:00"],
    [3_723_000, "1:02:03"],
  ])("%i ms -> %s", (ms, want) => expect(formatDuration(ms)).toBe(want));

  it.each([[null], [undefined], [-1], [NaN], [Infinity]])("shows a placeholder for %s", (v) =>
    expect(formatDuration(v as number | null)).toBe("--:--"),
  );
});

describe("track helpers", () => {
  it("falls back from title to file name to id", () => {
    expect(trackTitle(makeTrack({ title: "  Grenade  " }))).toBe("Grenade");
    expect(trackTitle(makeTrack({ title: "", path: "/a/b/song.m4a" }))).toBe("song.m4a");
    expect(trackTitle(makeTrack({ id: 7, title: null as never, path: "" }))).toBe("Track 7");
  });

  it("is playable only when decodable and present, with an honest reason", () => {
    expect(isPlayable(makeTrack())).toBe(true);
    const missing = makeTrack({ missing: true });
    expect(isPlayable(missing)).toBe(false);
    expect(unplayableReason(missing)).toMatch(/missing/i);
    const iso = makeTrack({ decodable: false, format: "sacd_iso" });
    expect(isPlayable(iso)).toBe(false);
    expect(unplayableReason(iso)).toMatch(/extraction/i);
    expect(unplayableReason(makeTrack())).toBe("");
  });

  it("builds a format badge with bit depth and rate", () => {
    expect(formatBadge(makeTrack({ format: "m4a", bit_depth: 24, sample_rate: 96000 }))).toBe("M4A · 24/96k");
    expect(formatBadge(makeTrack({ format: "flac", bit_depth: 16, sample_rate: 44100 }))).toBe("FLAC · 16/44.1k");
    expect(formatBadge(makeTrack({ format: "mp3", bit_depth: null, sample_rate: 48000 }))).toBe("MP3 · 48kHz");
    expect(formatBadge(makeTrack({ format: "sacd_iso", bit_depth: null, sample_rate: null }))).toBe("SACD ISO");
  });

  it("offers the right per-track formats", () => {
    expect(validFormatsFor(makeTrack({ format: "dsf" }))).toEqual(["flac", "dop"]);
    expect(validFormatsFor(makeTrack({ format: "sacd_iso" }))).toEqual(["flac"]);
    expect(validFormatsFor(makeTrack({ format: "flac" }))).toEqual(["passthrough", "flac", "opus", "mp3"]);
  });
});
