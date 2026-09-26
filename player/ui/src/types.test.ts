import { describe, expect, it } from "vitest";
import { makeTrack } from "./test/fixtures";
import {
  formatBadge,
  formatDuration,
  isPlayable,
  mqaLabel,
  mqaTitle,
  qualityTitle,
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

describe("MQA labelling", () => {
  it("shows the master's rate when the file says, plain MQA otherwise", () => {
    expect(mqaLabel(makeTrack({ mqa: true, original_sample_rate: 48000 }))).toBe("MQA · 48k");
    expect(mqaLabel(makeTrack({ mqa: true, original_sample_rate: 44100 }))).toBe("MQA · 44.1k");
    expect(mqaLabel(makeTrack({ mqa: true, original_sample_rate: 192000 }))).toBe("MQA · 192k");
    expect(mqaLabel(makeTrack({ mqa: true, original_sample_rate: null }))).toBe("MQA");
  });

  it("explains what the badge means without overpromising", () => {
    const tip = mqaTitle(makeTrack({ mqa: true, original_sample_rate: 96000 }));
    expect(tip).toContain("master 96 kHz");
    expect(tip).toContain("plays as ordinary FLAC");
    expect(tip).toContain("Bit-perfect output");
    expect(mqaTitle(makeTrack({ mqa: true }))).not.toContain("master");
  });
});

describe("qualityTitle (badge tooltip)", () => {
  it("spells out bit depth, rate, bitrate and channels", () => {
    expect(qualityTitle(makeTrack({ format: "m4a", bit_depth: 24, sample_rate: 96000, bitrate: 2647, channels: 2 }))).toBe(
      "M4A · 24-bit / 96 kHz · 2647 kbps · 2 ch",
    );
    expect(qualityTitle(makeTrack({ format: "flac", bit_depth: 16, sample_rate: 44100, bitrate: 900, channels: 2 }))).toBe(
      "FLAC · 16-bit / 44.1 kHz · 900 kbps · 2 ch",
    );
  });

  it("copes with lossy files (no bit depth) and missing fields", () => {
    expect(qualityTitle(makeTrack({ format: "mp3", bit_depth: null, sample_rate: 48000, bitrate: 320, channels: 2 }))).toBe(
      "MP3 · 48 kHz · 320 kbps · 2 ch",
    );
    expect(qualityTitle(makeTrack({ format: "sacd_iso", bit_depth: null, sample_rate: null, bitrate: null, channels: null }))).toBe("SACD ISO");
    expect(qualityTitle(makeTrack({ format: "dsf", bit_depth: 1, sample_rate: 2822400, bitrate: 5644, channels: 2 }))).toBe(
      "DSF · 1-bit / 2822.4 kHz · 5644 kbps · 2 ch",
    );
  });
});
