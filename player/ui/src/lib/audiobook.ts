// Pure rules for audiobooks. A position is always a *book offset*: ms from
// the start of the book. These turn it into (file, place in file) and back,
// find chapters, group history by day, and shape the sleep timer's fade.

import type { AudiobookChapter, AudiobookPart, AudiobookSession } from "../types";

/** Speeds offered in the UI (the engine accepts 0.5 to 3.0). */
export const SPEEDS = [0.75, 1, 1.25, 1.5, 1.75, 2, 2.25, 2.5] as const;
export const SPEED_RANGE = [0.5, 3.0] as const;
export const SKIP_DEFAULTS = { back: 15, forward: 30 } as const;
export const SLEEP_MINUTES = [5, 10, 15, 30, 45, 60] as const;
/** The sleep timer fades the sound out over this long before it pauses. */
export const SLEEP_FADE_MS = 10_000;
/** A restart this close to the end begins the book again. */
export const FINISHED_SHARE = 0.97;

export function clampSpeed(v: number): number {
  if (!Number.isFinite(v)) return 1;
  return Math.min(SPEED_RANGE[1], Math.max(SPEED_RANGE[0], Math.round(v * 100) / 100));
}

/** "1.5×", "1×", "0.75×". */
export function speedLabel(v: number): string {
  return `${Number.isInteger(v) ? v : v.toString()}×`;
}

/** "1:02:03", or "4:05" under an hour. */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s / 60) % 60);
  const sec = s % 60;
  const pad = (n: number): string => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}

/** "3 h 12 min", "47 min", "under a minute". */
export function duration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 60_000));
  if (total < 1) return "under a minute";
  const h = Math.floor(total / 60);
  const m = total % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/** Time left at the current speed: "3 h 12 min left". */
export function remainingText(durationMs: number, offsetMs: number, speed = 1): string {
  const left = Math.max(0, durationMs - offsetMs) / Math.max(0.5, speed);
  return `${duration(left)} left`;
}

/** The book offset a playing file position stands for. `null` when the file
 *  is not one of the book's parts. */
export function bookOffset(parts: AudiobookPart[], trackId: number | null | undefined, positionMs: number): number | null {
  if (trackId == null) return null;
  const p = parts.find((x) => x.track_id === trackId);
  return p ? p.start_offset_ms + Math.max(0, positionMs) : null;
}

export interface Resolved {
  partIndex: number;
  trackId: number;
  trackOffsetMs: number;
}

/** Which file, and where in it, a book offset falls (the server's
 *  `resolve` rule, for seeking without a round trip). */
export function resolveOffset(parts: AudiobookPart[], offsetMs: number): Resolved | null {
  if (parts.length === 0) return null;
  const at = Math.max(0, offsetMs);
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i];
    if (at < p.start_offset_ms + p.duration_ms) {
      return { partIndex: i, trackId: p.track_id, trackOffsetMs: Math.max(0, at - p.start_offset_ms) };
    }
  }
  const last = parts[parts.length - 1];
  return { partIndex: parts.length - 1, trackId: last.track_id, trackOffsetMs: last.duration_ms };
}

/** Index of the chapter containing `offsetMs`, or -1 when there are none. */
export function chapterIndexAt(chapters: AudiobookChapter[], offsetMs: number): number {
  let found = -1;
  for (let i = 0; i < chapters.length; i++) {
    if (chapters[i].start_offset_ms <= offsetMs) found = i;
    else break;
  }
  return found;
}

/** Where "next chapter" goes: the start of the following chapter, or null at the last. */
export function nextChapterStart(chapters: AudiobookChapter[], offsetMs: number): number | null {
  const i = chapterIndexAt(chapters, offsetMs);
  const next = chapters[i + 1];
  return next ? next.start_offset_ms : null;
}

/** Where "previous chapter" goes: the start of this chapter, unless you are
 *  within `restartWithinMs` of it, then the one before (so a double press
 *  steps back). */
export function prevChapterStart(chapters: AudiobookChapter[], offsetMs: number, restartWithinMs = 3000): number {
  const i = chapterIndexAt(chapters, offsetMs);
  if (i < 0) return 0;
  const here = chapters[i].start_offset_ms;
  if (offsetMs - here > restartWithinMs || i === 0) return here;
  return chapters[i - 1].start_offset_ms;
}

/** Skip by `deltaMs`, kept inside the book. */
export function skipTarget(offsetMs: number, deltaMs: number, durationMs: number): number {
  return Math.min(Math.max(0, durationMs), Math.max(0, offsetMs + deltaMs));
}

/** A saved position to start from: not at the very end of a finished book. */
export function startOffset(positionMs: number, durationMs: number): number {
  if (durationMs > 0 && positionMs >= FINISHED_SHARE * durationMs) return 0;
  return Math.max(0, positionMs);
}

// --- History ----------------------------------------------------------------

export interface DayGroup {
  /** Local calendar day, "2026-09-30". */
  day: string;
  /** "Tue Sep 30". */
  label: string;
  /** Where the newest session of the day stopped. */
  stoppedAtMs: number;
  /** Book time listened that day. */
  listenedMs: number;
  sessions: AudiobookSession[];
}

function localDay(ms: number): string {
  const d = new Date(ms);
  const p = (n: number): string => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Group sessions (as the server sends them, newest first) by local day. */
export function groupByDay(sessions: AudiobookSession[], locale?: string): DayGroup[] {
  const groups: DayGroup[] = [];
  for (const s of sessions) {
    const day = localDay(s.started_at);
    let g = groups.find((x) => x.day === day);
    if (!g) {
      g = {
        day,
        label: new Date(s.started_at).toLocaleDateString(locale, { weekday: "short", month: "short", day: "numeric" }),
        stoppedAtMs: s.end_offset_ms,
        listenedMs: 0,
        sessions: [],
      };
      groups.push(g);
    }
    g.listenedMs += s.listened_ms;
    g.sessions.push(s);
  }
  return groups;
}

/** "Tue Sep 30: stopped at 4:12:33, 47 min listened". */
export function dayLine(g: DayGroup): string {
  return `${g.label}: stopped at ${clock(g.stoppedAtMs)}, ${duration(g.listenedMs)} listened`;
}

// --- Sleep timer ------------------------------------------------------------

/** Volume factor (0 to 1) for the last `fadeMs` before the timer fires. */
export function sleepGain(remainingMs: number, fadeMs = SLEEP_FADE_MS): number {
  if (remainingMs >= fadeMs) return 1;
  return Math.min(1, Math.max(0, remainingMs / fadeMs));
}

/** Wall-clock ms until a point `mediaMsAhead` away is reached at `speed`. */
export function wallMs(mediaMsAhead: number, speed: number): number {
  return Math.max(0, mediaMsAhead) / Math.max(0.5, speed);
}
