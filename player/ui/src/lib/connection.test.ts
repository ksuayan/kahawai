import { describe, expect, it } from "vitest";
import { connectionHealth, formatAhead, formatRate, gaugeFill } from "./connection";

describe("formatRate", () => {
  it("shows kbit/s below a megabit and Mbit/s above", () => {
    expect(formatRate(50_000)).toBe("400 kbit/s");
    expect(formatRate(1_250_000)).toBe("10.0 Mbit/s");
    expect(formatRate(4_375_000)).toBe("35.0 Mbit/s");
    expect(formatRate(30_000_000)).toBe("240 Mbit/s"); // no decimals once it is large
  });
  it("is a dash when not measured", () => {
    for (const v of [null, undefined, 0, -5, Number.NaN]) expect(formatRate(v)).toBe("—");
  });
});

describe("formatAhead", () => {
  it("is seconds, then minutes", () => {
    expect(formatAhead(0)).toBe("0 s");
    expect(formatAhead(18_400)).toBe("18 s");
    expect(formatAhead(59_999)).toBe("59 s");
    expect(formatAhead(60_000)).toBe("1 min");
    expect(formatAhead(725_000)).toBe("12 min");
    expect(formatAhead(null)).toBe("—");
  });
});

describe("connectionHealth and gaugeFill", () => {
  it("grades the buffer", () => {
    expect(connectionHealth(25_000)).toBe("good");
    expect(connectionHealth(10_000)).toBe("good");
    expect(connectionHealth(9_999)).toBe("fair");
    expect(connectionHealth(3_000)).toBe("fair");
    expect(connectionHealth(2_999)).toBe("low");
    expect(connectionHealth(0)).toBe("low");
    expect(connectionHealth(null)).toBe("unknown");
  });
  it("calls a completely fetched stream good however little is left", () => {
    expect(connectionHealth(1_200, true)).toBe("good");
    expect(connectionHealth(null, true)).toBe("good");
    expect(gaugeFill(1_200, true)).toBe(1);
  });
  it("fills the bar up to 30 s", () => {
    expect(gaugeFill(0)).toBe(0);
    expect(gaugeFill(15_000)).toBe(0.5);
    expect(gaugeFill(90_000)).toBe(1);
    expect(gaugeFill(null)).toBe(0);
  });
});
