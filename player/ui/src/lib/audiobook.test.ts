import { describe, expect, it } from "vitest";
import type { AudiobookChapter, AudiobookPart, AudiobookSession } from "../types";
import {
  bookOffset,
  chapterIndexAt,
  clampSpeed,
  clock,
  dayLine,
  duration,
  groupByDay,
  nextChapterStart,
  prevChapterStart,
  remainingText,
  resolveOffset,
  skipTarget,
  sleepGain,
  speedLabel,
  startOffset,
  wallMs,
} from "./audiobook";

const parts: AudiobookPart[] = [
  { id: 1, track_id: 10, part_index: 0, title: "One", start_offset_ms: 0, duration_ms: 1000 },
  { id: 2, track_id: 11, part_index: 1, title: "Two", start_offset_ms: 1000, duration_ms: 2000 },
  { id: 3, track_id: 12, part_index: 2, title: "Three", start_offset_ms: 3000, duration_ms: 500 },
];
const chapters: AudiobookChapter[] = [
  { id: 1, part_id: 1, title: "A", start_offset_ms: 0, duration_ms: 1000 },
  { id: 2, part_id: 2, title: "B", start_offset_ms: 1000, duration_ms: 1200 },
  { id: 3, part_id: 2, title: "C", start_offset_ms: 2200, duration_ms: 800 },
];

describe("formatting", () => {
  it("clocks and durations", () => {
    expect(clock(0)).toBe("0:00");
    expect(clock(65_000)).toBe("1:05");
    expect(clock(4 * 3600_000 + 12 * 60_000 + 33_000)).toBe("4:12:33");
    expect(duration(20_000)).toBe("under a minute");
    expect(duration(47 * 60_000)).toBe("47 min");
    expect(duration(3 * 3600_000)).toBe("3 h");
    expect(duration(3 * 3600_000 + 12 * 60_000)).toBe("3 h 12 min");
  });
  it("time left is at the playing speed", () => {
    expect(remainingText(2 * 3600_000, 0, 2)).toBe("1 h left");
    expect(remainingText(100_000, 100_000)).toBe("under a minute left");
  });
  it("speeds are clamped and labelled", () => {
    expect(clampSpeed(9)).toBe(3);
    expect(clampSpeed(0.1)).toBe(0.5);
    expect(clampSpeed(NaN)).toBe(1);
    expect(clampSpeed(1.2345)).toBe(1.23);
    expect(speedLabel(1)).toBe("1×");
    expect(speedLabel(1.25)).toBe("1.25×");
  });
});

describe("offsets", () => {
  it("a file position is a book offset", () => {
    expect(bookOffset(parts, 11, 250)).toBe(1250);
    expect(bookOffset(parts, 99, 250)).toBeNull();
    expect(bookOffset(parts, null, 250)).toBeNull();
  });
  it("a book offset resolves to a file and a place in it", () => {
    expect(resolveOffset(parts, 0)).toEqual({ partIndex: 0, trackId: 10, trackOffsetMs: 0 });
    expect(resolveOffset(parts, 999)?.trackOffsetMs).toBe(999);
    expect(resolveOffset(parts, 1000)).toEqual({ partIndex: 1, trackId: 11, trackOffsetMs: 0 });
    expect(resolveOffset(parts, 3200)).toEqual({ partIndex: 2, trackId: 12, trackOffsetMs: 200 });
    expect(resolveOffset(parts, 99_999)).toEqual({ partIndex: 2, trackId: 12, trackOffsetMs: 500 });
    expect(resolveOffset(parts, -5)?.partIndex).toBe(0);
    expect(resolveOffset([], 5)).toBeNull();
  });
  it("skips stay inside the book", () => {
    expect(skipTarget(500, -15_000, 10_000)).toBe(0);
    expect(skipTarget(9_000, 30_000, 10_000)).toBe(10_000);
    expect(skipTarget(5_000, 1_000, 10_000)).toBe(6_000);
  });
  it("a book near its end starts again", () => {
    expect(startOffset(5_000, 100_000)).toBe(5_000);
    expect(startOffset(98_000, 100_000)).toBe(0);
    expect(startOffset(-4, 100_000)).toBe(0);
  });
});

describe("chapters", () => {
  it("finds the chapter at an offset", () => {
    expect(chapterIndexAt(chapters, 0)).toBe(0);
    expect(chapterIndexAt(chapters, 1500)).toBe(1);
    expect(chapterIndexAt(chapters, 2200)).toBe(2);
    expect(chapterIndexAt(chapters, 99_999)).toBe(2);
    expect(chapterIndexAt([], 5)).toBe(-1);
  });
  it("next and previous", () => {
    expect(nextChapterStart(chapters, 500)).toBe(1000);
    expect(nextChapterStart(chapters, 2500)).toBeNull();
    expect(prevChapterStart(chapters, 1500, 300)).toBe(1000);
    expect(prevChapterStart(chapters, 1500)).toBe(0); // within 3 s of the start: the one before
    expect(prevChapterStart(chapters, 1100)).toBe(0);
    expect(prevChapterStart(chapters, 100)).toBe(0);
    expect(prevChapterStart([], 100)).toBe(0);
  });
});

describe("history by day", () => {
  const at = (day: number, h: number): number => new Date(2026, 8, day, h, 0, 0).getTime();
  const s = (id: number, started: number, a: number, b: number): AudiobookSession => ({
    id,
    started_at: started,
    ended_at: started + 60_000,
    start_offset_ms: a,
    end_offset_ms: b,
    listened_ms: Math.max(0, b - a),
  });
  it("groups newest-first sessions by local day with where you stopped", () => {
    const sessions = [
      s(4, at(30, 21), 14_000_000, 15_153_000), // newest, Sep 30 evening
      s(3, at(30, 8), 12_000_000, 12_600_000),
      s(2, at(29, 22), 9_000_000, 9_500_000),
    ];
    const g = groupByDay(sessions, "en-US");
    expect(g).toHaveLength(2);
    expect(g[0].stoppedAtMs).toBe(15_153_000);
    expect(g[0].listenedMs).toBe(1_153_000 + 600_000);
    expect(g[0].sessions).toHaveLength(2);
    expect(dayLine(g[0])).toBe("Wed, Sep 30: stopped at 4:12:33, 29 min listened");
    expect(g[1].sessions).toHaveLength(1);
  });
  it("is empty for no sessions", () => {
    expect(groupByDay([])).toEqual([]);
  });
});

describe("sleep timer", () => {
  it("holds the volume, then fades linearly over the last 10 s", () => {
    expect(sleepGain(60_000)).toBe(1);
    expect(sleepGain(10_000)).toBe(1);
    expect(sleepGain(5_000)).toBeCloseTo(0.5);
    expect(sleepGain(0)).toBe(0);
    expect(sleepGain(-3)).toBe(0);
  });
  it("end of chapter is wall-clock at the speed", () => {
    expect(wallMs(60_000, 2)).toBe(30_000);
    expect(wallMs(-5, 1)).toBe(0);
  });
});
