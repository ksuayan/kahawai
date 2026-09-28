import { describe, expect, it } from "vitest";
import { computeGridLayout, computeLanes, maxRealizedRows, rowItemIds } from "./gridwindowing";

const GAP = 16;

describe("computeLanes", () => {
  it("fits as many 160px-min cells as possible", () => {
    // (1200 + 16) / (160 + 16) = 6.9 -> 6 lanes
    expect(computeLanes(1200, 160, GAP)).toBe(6);
  });

  it("never returns zero lanes", () => {
    expect(computeLanes(0, 160, GAP)).toBe(1);
    expect(computeLanes(100, 160, GAP)).toBe(1);
  });

  it("grows lanes as the container widens", () => {
    const small = computeLanes(800, 160, GAP);
    const large = computeLanes(1600, 160, GAP);
    expect(large).toBeGreaterThan(small);
  });

  it("shrinks lanes when the minimum cell width grows", () => {
    const small = computeLanes(1200, 130, GAP);
    const large = computeLanes(1200, 260, GAP);
    expect(small).toBeGreaterThan(large);
  });
});

describe("computeGridLayout", () => {
  it("produces cells that exactly fill the row", () => {
    const l = computeGridLayout(1200, 100, 160, GAP);
    expect(l.lanes * l.cellWidth + (l.lanes - 1) * GAP).toBeCloseTo(1200, 6);
    expect(l.rowHeight).toBe(l.cellWidth + GAP);
  });

  it("rounds up the row count", () => {
    const l = computeGridLayout(1200, 17, 160, GAP);
    expect(l.rowCount).toBe(Math.ceil(17 / l.lanes));
  });

  it("handles an empty grid", () => {
    const l = computeGridLayout(1200, 0, 160, GAP);
    expect(l.rowCount).toBe(0);
    expect(l.lanes).toBeGreaterThan(0);
  });

  it("reports zero cell width and lane 1 for an unmeasured (zero-width) container", () => {
    const l = computeGridLayout(0, 50, 160, GAP);
    expect(l.lanes).toBe(1);
    expect(l.cellWidth).toBe(0);
  });
});

describe("rowItemIds", () => {
  const ids = [1, 2, 3, 4, 5, 6, 7];

  it("slices lanes-wide windows", () => {
    expect(rowItemIds(0, 3, ids)).toEqual([1, 2, 3]);
    expect(rowItemIds(1, 3, ids)).toEqual([4, 5, 6]);
  });

  it("clamps the final partial row", () => {
    expect(rowItemIds(2, 3, ids)).toEqual([7]);
  });

  it("returns empty past the end", () => {
    expect(rowItemIds(9, 3, ids)).toEqual([]);
  });
});

describe("maxRealizedRows", () => {
  it("bounds realized rows by viewport + overscan, not item count", () => {
    const rowHeight = 160 + GAP;
    const bound = maxRealizedRows(900, rowHeight, 4);
    // ceil(900/176) + 1 (mid-row offset) + 8 = 6 + 1 + 8 = 15 rows max,
    // for ANY item count.
    expect(bound).toBe(15);
    expect(bound).toBeLessThan(50);
  });
});
