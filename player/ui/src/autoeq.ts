import { EQ_LIMITS } from "./eqResponse";
import { EQ_PREAMP_RANGE_DB, MAX_EQ_BANDS, type EqBand } from "./types";

/** Result of parsing an AutoEq-style parametric EQ text file. */
export interface AutoEqProfile {
  preamp_db: number;
  bands: EqBand[];
  /** Things that were adjusted or skipped; shown in the import preview. */
  warnings: string[];
}

const DEFAULT_Q = 0.707;
const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));
const round2 = (n: number): number => Math.round(n * 100) / 100;

/**
 * AutoEq (and Equalizer APO) describe shelves by RBJ Q; Kahawai's shelves take
 * the RBJ *slope* S. Solving the cookbook's alpha for both forms gives
 * `1/Q² = (A + 1/A)(1/S − 1) + 2`, hence the conversion below.
 */
export function shelfQToSlope(q: number, gain_db: number): number {
  const a = 10 ** (gain_db / 40);
  const s = 1 / ((1 / (q * q) - 2) / (a + 1 / a) + 1);
  return clamp(s, EQ_LIMITS.slopeMin, EQ_LIMITS.slopeMax);
}

const FILTER_RE = /^filter\s*\d*\s*:\s*(on|off)\s+(\S+)\s*(.*)$/i;

function num(rest: string, key: string): number | null {
  const m = new RegExp(`\\b${key}\\s+(-?\\d+(?:[.,]\\d+)?)`, "i").exec(rest);
  return m ? Number(m[1].replace(",", ".")) : null;
}

/**
 * Parse `ParametricEQ.txt` content:
 *
 *     Preamp: -6.2 dB
 *     Filter 1: ON PK Fc 31 Hz Gain 5.5 dB Q 1.00
 *     Filter 2: ON LSC Fc 105 Hz Gain 7.0 dB Q 0.70
 *
 * Disabled (OFF) filters are dropped. Unsupported lines never throw; they
 * become warnings. Returns null when nothing usable was found.
 */
export function parseAutoEq(text: string): AutoEqProfile | null {
  const warnings: string[] = [];
  const bands: EqBand[] = [];
  let preamp = 0;
  let sawPreamp = false;

  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;

    const pre = /^preamp\s*:\s*(-?\d+(?:[.,]\d+)?)\s*db/i.exec(line);
    if (pre) {
      preamp = Number(pre[1].replace(",", "."));
      sawPreamp = true;
      continue;
    }

    const m = FILTER_RE.exec(line);
    if (!m) continue;
    const [, state, kind, rest] = m;
    if (state.toUpperCase() === "OFF") continue;

    const freq = num(rest, "fc");
    if (freq === null) {
      warnings.push(`Skipped “${line}”: no frequency.`);
      continue;
    }
    const gain = num(rest, "gain") ?? 0;
    const qRaw = num(rest, "q");
    if (qRaw === null && /\bbw\b/i.test(rest)) {
      warnings.push(`Filter at ${freq} Hz uses a bandwidth instead of Q; Q ${DEFAULT_Q} assumed.`);
    }
    const q = qRaw ?? DEFAULT_Q;

    let band: EqBand;
    switch (kind.toUpperCase()) {
      case "PK":
      case "PEQ":
        band = { band_type: "peaking", freq, gain_db: gain, q };
        break;
      case "LSC":
      case "LS":
        band = { band_type: "low_shelf", freq, gain_db: gain, q: shelfQToSlope(q, gain) };
        break;
      case "HSC":
      case "HS":
        band = { band_type: "high_shelf", freq, gain_db: gain, q: shelfQToSlope(q, gain) };
        break;
      case "LP":
      case "LPQ":
        band = { band_type: "low_pass", freq, gain_db: 0, q };
        break;
      case "HP":
      case "HPQ":
        band = { band_type: "high_pass", freq, gain_db: 0, q };
        break;
      default:
        warnings.push(`Skipped unsupported filter type ${kind} at ${freq} Hz.`);
        continue;
    }
    bands.push(band);
  }

  if (bands.length === 0 && !sawPreamp) return null;

  if (bands.length > MAX_EQ_BANDS) {
    warnings.push(`${bands.length} filters found; only the first ${MAX_EQ_BANDS} were kept.`);
    bands.length = MAX_EQ_BANDS;
  }

  const fixed = bands.map((b, i) => {
    const gHas = b.band_type === "peaking" || b.band_type === "low_shelf" || b.band_type === "high_shelf";
    const qMin = b.band_type === "low_shelf" || b.band_type === "high_shelf" ? EQ_LIMITS.slopeMin : EQ_LIMITS.qMin;
    const qMax = b.band_type === "low_shelf" || b.band_type === "high_shelf" ? EQ_LIMITS.slopeMax : EQ_LIMITS.qMax;
    const out: EqBand = {
      ...b,
      freq: round2(clamp(b.freq, EQ_LIMITS.freqMin, EQ_LIMITS.freqMax)),
      gain_db: gHas ? round2(clamp(b.gain_db, -EQ_LIMITS.gainMax, EQ_LIMITS.gainMax)) : 0,
      q: round2(clamp(b.q, qMin, qMax)),
    };
    if (out.freq !== round2(b.freq) || (gHas && out.gain_db !== round2(b.gain_db))) {
      warnings.push(`Filter ${i + 1} was clamped to ${out.freq} Hz / ${out.gain_db} dB (Kahawai limits).`);
    }
    return out;
  });

  const [lo, hi] = EQ_PREAMP_RANGE_DB;
  const preampFixed = round2(clamp(preamp, lo, hi));
  if (preampFixed !== round2(preamp)) warnings.push(`Preamp ${preamp} dB was clamped to ${preampFixed} dB.`);

  return { preamp_db: preampFixed, bands: fixed, warnings };
}
