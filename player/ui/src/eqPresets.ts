import type { EqBand } from "./types";

export interface EqPreset {
  /** Stable id: `builtin:<slug>` or `user:<name>`. */
  id: string;
  name: string;
  bands: EqBand[];
  builtin: boolean;
}

const peak = (freq: number, gain_db: number, q = 1): EqBand => ({ band_type: "peaking", freq, gain_db, q });
const lowShelf = (freq: number, gain_db: number): EqBand => ({ band_type: "low_shelf", freq, gain_db, q: 0.7 });
const highShelf = (freq: number, gain_db: number): EqBand => ({ band_type: "high_shelf", freq, gain_db, q: 0.7 });

/** Starting points, deliberately gentle (≤ ±4 dB); fine-tune in Settings. */
export const BUILTIN_PRESETS: EqPreset[] = [
  { id: "builtin:flat", name: "Flat", builtin: true, bands: [] },
  {
    id: "builtin:classical",
    name: "Classical",
    builtin: true,
    bands: [lowShelf(80, 2), peak(400, -1), highShelf(8000, 3)],
  },
  {
    id: "builtin:jazz",
    name: "Jazz",
    builtin: true,
    bands: [lowShelf(100, 2.5), peak(250, 1.5), peak(3000, 1.5), highShelf(10000, 2)],
  },
  {
    id: "builtin:rock",
    name: "Rock",
    builtin: true,
    bands: [lowShelf(80, 4), peak(500, -2), peak(3000, 2.5), highShelf(10000, 3)],
  },
  {
    id: "builtin:pop",
    name: "Pop",
    builtin: true,
    bands: [lowShelf(80, 2), peak(250, -1.5), peak(2000, 2.5), highShelf(8000, 1.5)],
  },
  {
    id: "builtin:talk-show",
    name: "Talk Show",
    builtin: true,
    bands: [
      { band_type: "high_pass", freq: 90, gain_db: 0, q: 0.7 },
      peak(200, -2),
      peak(3000, 4),
      peak(5500, 1.5),
    ],
  },
];

const round = (n: number): number => Math.round(n * 100) / 100;

/** Same tuning, ignoring row flags and float noise. */
export function sameBands(a: EqBand[], b: EqBand[]): boolean {
  return (
    a.length === b.length &&
    a.every(
      (x, i) =>
        x.band_type === b[i].band_type &&
        round(x.freq) === round(b[i].freq) &&
        round(x.gain_db) === round(b[i].gain_db) &&
        round(x.q) === round(b[i].q),
    )
  );
}
