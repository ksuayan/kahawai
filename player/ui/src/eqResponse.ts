import type { EqBand } from "./types";

/** Rate the curve is drawn for when no stream says otherwise. */
export const DEFAULT_RATE_HZ = 48000;

/** UI guardrails. Tighter than the engine's own limits (10–24000 Hz, ±24 dB) so a
 *  band can only be set to something that is meaningful and visible on the graph. */
export const EQ_LIMITS = {
  freqMin: 20,
  freqMax: 20000,
  gainMax: 18,
  qMin: 0.1,
  qMax: 18,
  slopeMin: 0.1,
  slopeMax: 3,
} as const;

/** Mirrors `usable_freq` in dsp.rs: the engine caps a band at 0.45 x the sample rate. */
export const NYQUIST_FRACTION = 0.45;
export const usableFreq = (freq: number, fs: number): number => Math.min(freq, fs * NYQUIST_FRACTION);

/** Highest frequency a band may be set to at this output rate. */
export const maxFreqFor = (fs: number): number => Math.floor(Math.min(EQ_LIMITS.freqMax, fs * NYQUIST_FRACTION));

export const bandHasGain = (t: EqBand["band_type"]): boolean => t === "peaking" || t === "low_shelf" || t === "high_shelf";
const isShelf = (t: EqBand["band_type"]): boolean => t === "low_shelf" || t === "high_shelf";
const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));

/** Range of the Q / slope field for a band type. */
export function qRange(t: EqBand["band_type"]): { min: number; max: number } {
  return isShelf(t) ? { min: EQ_LIMITS.slopeMin, max: EQ_LIMITS.slopeMax } : { min: EQ_LIMITS.qMin, max: EQ_LIMITS.qMax };
}

/** Pull a band back into the allowed ranges for this rate (returns a new band). */
export function constrainBand(b: EqBand, fs: number): EqBand {
  const q = qRange(b.band_type);
  return {
    ...b,
    freq: clamp(Number.isFinite(b.freq) ? b.freq : 1000, EQ_LIMITS.freqMin, maxFreqFor(fs)),
    gain_db: bandHasGain(b.band_type) ? clamp(Number.isFinite(b.gain_db) ? b.gain_db : 0, -EQ_LIMITS.gainMax, EQ_LIMITS.gainMax) : 0,
    q: clamp(Number.isFinite(b.q) ? b.q : 1, q.min, q.max),
  };
}

/** Magnitude (dB) of one band at `f` Hz for an output at `fs` Hz; mirrors `design_band` in kahawai-player-core (RBJ cookbook). */
export function bandResponseDb(band: EqBand, f: number, fs = DEFAULT_RATE_HZ): number {
  const a = 10 ** (band.gain_db / 40);
  const w0 = (2 * Math.PI * usableFreq(band.freq, fs)) / fs;
  const cw = Math.cos(w0);
  const sw = Math.sin(w0);
  const alpha = sw / (2 * band.q);
  let b0: number, b1: number, b2: number, a0: number, a1: number, a2: number;
  switch (band.band_type) {
    case "peaking":
      [b0, b1, b2, a0, a1, a2] = [1 + alpha * a, -2 * cw, 1 - alpha * a, 1 + alpha / a, -2 * cw, 1 - alpha / a];
      break;
    case "low_shelf":
    case "high_shelf": {
      const s = Math.min(3, Math.max(0.1, band.q));
      const alphaS = (sw / 2) * Math.sqrt((a + 1 / a) * (1 / s - 1) + 2);
      const sq = 2 * Math.sqrt(a) * alphaS;
      if (band.band_type === "low_shelf") {
        [b0, b1, b2, a0, a1, a2] = [
          a * (a + 1 - (a - 1) * cw + sq),
          2 * a * (a - 1 - (a + 1) * cw),
          a * (a + 1 - (a - 1) * cw - sq),
          a + 1 + (a - 1) * cw + sq,
          -2 * (a - 1 + (a + 1) * cw),
          a + 1 + (a - 1) * cw - sq,
        ];
      } else {
        [b0, b1, b2, a0, a1, a2] = [
          a * (a + 1 + (a - 1) * cw + sq),
          -2 * a * (a - 1 + (a + 1) * cw),
          a * (a + 1 + (a - 1) * cw - sq),
          a + 1 - (a - 1) * cw + sq,
          2 * (a - 1 - (a + 1) * cw),
          a + 1 - (a - 1) * cw - sq,
        ];
      }
      break;
    }
    case "low_pass": {
      const c = (1 - cw) / 2;
      [b0, b1, b2, a0, a1, a2] = [c, 1 - cw, c, 1 + alpha, -2 * cw, 1 - alpha];
      break;
    }
    case "high_pass": {
      const c = (1 + cw) / 2;
      [b0, b1, b2, a0, a1, a2] = [c, -(1 + cw), c, 1 + alpha, -2 * cw, 1 - alpha];
      break;
    }
  }
  const w = (2 * Math.PI * f) / fs;
  const c1 = Math.cos(w), s1 = Math.sin(w), c2 = Math.cos(2 * w), s2 = Math.sin(2 * w);
  const nr = b0 + b1 * c1 + b2 * c2, ni = -(b1 * s1 + b2 * s2);
  const dr = a0 + a1 * c1 + a2 * c2, di = -(a1 * s1 + a2 * s2);
  return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

/** Combined response (dB) of all bands at `f` Hz. */
export function totalResponseDb(bands: EqBand[], f: number, fs = DEFAULT_RATE_HZ): number {
  return bands.reduce((sum, b) => sum + bandResponseDb(b, f, fs), 0);
}

export type BandSeverity = "ok" | "warn" | "bad";

/** Thresholds for "this will sound bad". Kept together so they are easy to tune. */
export const SEVERITY = {
  boostWarnDb: 9,
  boostBadDb: 15,
  qWarn: 12,
  peakWarnDb: 6,
  peakBadDb: 10,
  nearCapFraction: 0.9,
} as const;

/**
 * How risky one band is for sound quality, and why (`null` reason when fine).
 * `peakDb` is the highest boost of the whole curve: any boosting band shares
 * the blame for clipping.
 */
export function bandSeverity(
  band: EqBand,
  fs: number,
  peakDb: number,
): { level: BandSeverity; reason: string | null } {
  const cap = fs * NYQUIST_FRACTION;
  const boosting = bandHasGain(band.band_type) && band.gain_db > 0;
  let level: BandSeverity = "ok";
  let reason: string | null = null;
  const raise = (l: BandSeverity, why: string): void => {
    if (l === "bad" || (l === "warn" && level === "ok")) {
      level = l;
      reason = why;
    }
  };
  if (band.freq > cap) raise("bad", `above the usable range at ${Math.round(fs / 100) / 10} kHz; applied at ${Math.floor(cap)} Hz`);
  else if (band.freq > cap * SEVERITY.nearCapFraction) raise("warn", "close to the top of the usable range; the shape gets distorted");
  if (boosting && band.gain_db >= SEVERITY.boostBadDb) raise("bad", `+${band.gain_db} dB boost is very large and will likely distort`);
  else if (boosting && band.gain_db >= SEVERITY.boostWarnDb) raise("warn", `+${band.gain_db} dB boost is large`);
  if (boosting && peakDb >= SEVERITY.peakBadDb) raise("bad", `the combined boost of +${peakDb.toFixed(1)} dB will clip loud tracks`);
  else if (boosting && peakDb > SEVERITY.peakWarnDb) raise("warn", `the combined boost of +${peakDb.toFixed(1)} dB can clip loud tracks`);
  if (band.band_type !== "low_shelf" && band.band_type !== "high_shelf" && band.q > SEVERITY.qWarn) raise("warn", `Q ${band.q} is very narrow and can ring`);
  return { level, reason };
}
