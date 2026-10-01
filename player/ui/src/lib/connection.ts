/** Formatting and health rules for the connection gauge. */

export type ConnectionHealth = "good" | "fair" | "low" | "unknown";

/** Buffer thresholds, in ms of audio ahead of the playhead. */
export const HEALTH_GOOD_MS = 10_000;
export const HEALTH_FAIR_MS = 3_000;
/** The gauge bar is full at this much audio buffered. */
export const GAUGE_FULL_MS = 30_000;

/** Network speed as `kbit/s` or `Mbit/s`; "—" when not measured yet. */
export function formatRate(bytesPerSec: number | null | undefined): string {
  if (bytesPerSec == null || !Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return "—";
  const kbit = (bytesPerSec * 8) / 1000;
  if (kbit < 1000) return `${Math.round(kbit)} kbit/s`;
  const mbit = kbit / 1000;
  return `${mbit >= 100 ? Math.round(mbit) : mbit.toFixed(1)} Mbit/s`;
}

/** Buffered audio as `N s` up to a minute, then `N min`; "—" when unknown. */
export function formatAhead(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return "—";
  const s = Math.floor(ms / 1000);
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min`;
}

/**
 * How comfortable the buffer is. A stream that is already completely fetched
 * is always good (its buffer is just the rest of the track, which shrinks
 * toward the end), however little is left.
 */
export function connectionHealth(aheadMs: number | null | undefined, complete = false): ConnectionHealth {
  if (complete) return "good";
  if (aheadMs == null || !Number.isFinite(aheadMs)) return "unknown";
  if (aheadMs >= HEALTH_GOOD_MS) return "good";
  if (aheadMs >= HEALTH_FAIR_MS) return "fair";
  return "low";
}

/** 0..1 fill of the gauge bar. A completely fetched stream reads full. */
export function gaugeFill(aheadMs: number | null | undefined, complete = false): number {
  if (complete) return 1;
  if (aheadMs == null || !Number.isFinite(aheadMs) || aheadMs <= 0) return 0;
  return Math.min(1, aheadMs / GAUGE_FULL_MS);
}
