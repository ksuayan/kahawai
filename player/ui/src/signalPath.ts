import { CROSSFEED_PRESET_INFO, formatBadge, type CrossfeedPreset, type Track } from "./types";

/** What the tag needs to know about playback (the player store satisfies this). */
export interface PathPlayer {
  activeFormat: string | null;
  isDopExclusive: boolean;
  isBitPerfect: boolean;
  isExclusive: boolean;
}

/** What the tag needs to know about the user's processing (the dsp store satisfies this). */
export interface PathDsp {
  crossfeed?: { enabled: boolean; preset: CrossfeedPreset };
  eqEnabled: boolean;
  activeBands: unknown[];
  loudnessEnabled: boolean;
  loudnessTarget: number;
}

/**
 * The composed `original → converted → output` tag, e.g.
 * "DSF · 1/2822.4k → DOP → Exclusive DoP · bit-perfect" or
 * "M4A · 24/96k → PASSTHROUGH → PCM shared · EQ 2 bands". Shared by the
 * Now Playing view and the live signal-path panel so they never disagree.
 */
export function audioPathLabel(t: Track | null, player: PathPlayer, dsp: PathDsp): string {
  if (!t) return "Nothing playing";
  const src = formatBadge(t);
  const stream = player.activeFormat ? player.activeFormat.toUpperCase() : "AUTO";
  const out = player.isDopExclusive
    ? "Exclusive DoP · bit-perfect"
    : player.isBitPerfect
      ? "Bit-perfect · exclusive"
      : "PCM shared";
  const dspBits: string[] = [];
  if (!player.isExclusive) {
    // Chain order: crossfeed runs before the EQ.
    if (dsp.crossfeed?.enabled) dspBits.push(`Crossfeed ${CROSSFEED_PRESET_INFO[dsp.crossfeed.preset].label}`);
    if (dsp.eqEnabled && dsp.activeBands.length > 0) dspBits.push(`EQ ${dsp.activeBands.length} bands`);
    if (dsp.loudnessEnabled) dspBits.push(`Loudness ${dsp.loudnessTarget} LUFS`);
  }
  return `${src} → ${stream} → ${out}${dspBits.length ? ` · ${dspBits.join(" · ")}` : ""}`;
}

export const isDsd = (t: Track | null): boolean => !!t && (t.format === "dsf" || t.format === "dff");

/** "44.1 kHz", "96 kHz", "192 kHz". */
export function fmtKHz(hz: number): string {
  return `${Math.round(hz / 100) / 10} kHz`;
}

/** DSD rates by name: 2 822 400 → "DSD64". */
export function dsdName(rate: number): string {
  const n = Math.round(rate / 44100);
  return n > 0 ? `DSD${n}` : "DSD";
}

export type LinkState = "match" | "carried" | "convert" | "none";
export interface Link {
  state: LinkState;
  text: string;
}

/**
 * How the file's rate relates to what the device is running at:
 * an exact match (nothing resampled), DSD carried natively inside PCM frames,
 * or converted (resampled / DSD decimated).
 */
export function rateLink(t: Track | null, outputHz: number | null, dopExclusive: boolean): Link {
  if (!t?.sample_rate || !outputHz) return { state: "none", text: "" };
  if (isDsd(t)) {
    return dopExclusive && outputHz === t.sample_rate / 16
      ? { state: "carried", text: "DoP" }
      : { state: "convert", text: "converted" };
  }
  return outputHz === t.sample_rate
    ? { state: "match", text: "matched" }
    : { state: "convert", text: "resampled" };
}

/** How the file's bit depth relates to the device's stream (`slotBits` = its integer width). */
export function depthLink(t: Track | null, slotBits: number, exclusive: boolean, float: boolean): Link {
  if (!t || !slotBits) return { state: "none", text: "" };
  if (isDsd(t)) {
    return exclusive
      ? { state: "carried", text: "packed" }
      : { state: "convert", text: "converted" };
  }
  if (!t.bit_depth) return { state: "none", text: "" };
  if (float) return { state: "convert", text: "mixer" };
  // A 32-bit slot carries a 24-bit word, so up to 24 bits are kept whole.
  const usable = slotBits >= 32 ? 24 : slotBits;
  return t.bit_depth <= usable
    ? { state: "match", text: exclusive ? "bit-perfect" : "lossless" }
    : { state: "convert", text: "reduced" };
}
