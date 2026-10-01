import { describe, expect, it } from "vitest";
import { parseAutoEq, shelfQToSlope } from "./autoeq";
import { bandResponseDb } from "./eqResponse";

const SAMPLE = `Preamp: -6.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 7.0 dB Q 0.70
Filter 2: ON PK Fc 31 Hz Gain 5.5 dB Q 1.00
Filter 3: OFF PK Fc 500 Hz Gain 1.0 dB Q 1.00
Filter 4: ON HSC Fc 10000 Hz Gain -2.5 dB Q 0.70
`;

/** |H| in dB of an RBJ low shelf designed from Q (the reference AutoEq targets). */
function lowShelfQDb(f0: number, gainDb: number, q: number, f: number, fs: number): number {
  const A = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * f0) / fs;
  const cw = Math.cos(w0);
  const alpha = Math.sin(w0) / (2 * q);
  const k = 2 * Math.sqrt(A) * alpha;
  const b = [A * (A + 1 - (A - 1) * cw + k), 2 * A * (A - 1 - (A + 1) * cw), A * (A + 1 - (A - 1) * cw - k)];
  const a = [A + 1 + (A - 1) * cw + k, -2 * (A - 1 + (A + 1) * cw), A + 1 + (A - 1) * cw - k];
  const w = (2 * Math.PI * f) / fs;
  const ev = (c: number[]) => {
    const re = c[0] + c[1] * Math.cos(w) + c[2] * Math.cos(2 * w);
    const im = -(c[1] * Math.sin(w) + c[2] * Math.sin(2 * w));
    return re * re + im * im;
  };
  return 10 * Math.log10(ev(b) / ev(a));
}

describe("parseAutoEq", () => {
  it("reads preamp and filters, dropping OFF ones", () => {
    const p = parseAutoEq(SAMPLE)!;
    expect(p.preamp_db).toBe(-6.2);
    expect(p.bands.map((b) => b.band_type)).toEqual(["low_shelf", "peaking", "high_shelf"]);
    expect(p.bands[1]).toMatchObject({ freq: 31, gain_db: 5.5, q: 1 });
    expect(p.warnings).toEqual([]);
  });

  it("converts shelf Q to the slope the engine uses (Q 1/√2 → slope 1)", () => {
    expect(shelfQToSlope(Math.SQRT1_2, 7)).toBeCloseTo(1, 6);
    expect(shelfQToSlope(Math.SQRT1_2, -3)).toBeCloseTo(1, 6);
  });

  it("matches the Q-designed reference shelf across the spectrum", () => {
    const p = parseAutoEq("Filter 1: ON LSC Fc 105 Hz Gain 7.0 dB Q 0.50")!;
    for (const f of [30, 80, 105, 200, 1000, 8000]) {
      expect(bandResponseDb(p.bands[0], f, 48000)).toBeCloseTo(lowShelfQDb(105, 7, 0.5, f, 48000), 1);
    }
  });

  it("handles CRLF, decimal commas and a missing Q", () => {
    const p = parseAutoEq("Preamp: -1,5 dB\r\nFilter 1: ON PK Fc 1000 Hz Gain 2,5 dB\r\n")!;
    expect(p.preamp_db).toBe(-1.5);
    expect(p.bands[0]).toMatchObject({ gain_db: 2.5, q: 0.71 });
  });

  it("warns about unsupported types and caps the band count", () => {
    const many = Array.from({ length: 14 }, (_, i) => `Filter ${i + 1}: ON PK Fc ${100 + i * 100} Hz Gain 1 dB Q 1`).join("\n");
    const p = parseAutoEq(`${many}\nFilter 15: ON NOTCH Fc 50 Hz`)!;
    expect(p.bands).toHaveLength(12);
    expect(p.warnings.some((w) => w.includes("NOTCH"))).toBe(true);
    expect(p.warnings.some((w) => w.includes("only the first 12"))).toBe(true);
  });

  it("clamps out-of-range gain and preamp with a warning", () => {
    const p = parseAutoEq("Preamp: -40 dB\nFilter 1: ON PK Fc 1000 Hz Gain 22 dB Q 1")!;
    expect(p.preamp_db).toBe(-24);
    expect(p.bands[0].gain_db).toBe(18);
    expect(p.warnings).toHaveLength(2);
  });

  it("returns null for unrelated text", () => {
    expect(parseAutoEq("hello\nworld")).toBeNull();
  });
});
