import type { EqBand } from "./types";

/** Sample rate the curve is drawn for (the engine designs at the stream's own rate). */
const FS = 48000;

/** Magnitude (dB) of one band at `f` Hz; mirrors `design_band` in kahawai-player-core (RBJ cookbook). */
export function bandResponseDb(band: EqBand, f: number): number {
  const a = 10 ** (band.gain_db / 40);
  const w0 = (2 * Math.PI * band.freq) / FS;
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
  const w = (2 * Math.PI * f) / FS;
  const c1 = Math.cos(w), s1 = Math.sin(w), c2 = Math.cos(2 * w), s2 = Math.sin(2 * w);
  const nr = b0 + b1 * c1 + b2 * c2, ni = -(b1 * s1 + b2 * s2);
  const dr = a0 + a1 * c1 + a2 * c2, di = -(a1 * s1 + a2 * s2);
  return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

/** Combined response (dB) of all bands at `f` Hz. */
export function totalResponseDb(bands: EqBand[], f: number): number {
  return bands.reduce((sum, b) => sum + bandResponseDb(b, f), 0);
}
