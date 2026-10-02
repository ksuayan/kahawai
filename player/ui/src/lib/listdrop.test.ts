import { describe, expect, it } from "vitest";
import { isNoopSlot, partingFor, slotFromY, targetIndex } from "./listdrop";

describe("list drop geometry", () => {
  it("finds the nearest boundary between rows, clamped to the list", () => {
    expect(slotFromY(0, 50, 4)).toBe(0);
    expect(slotFromY(24, 50, 4)).toBe(0);
    expect(slotFromY(26, 50, 4)).toBe(1);
    expect(slotFromY(149, 50, 4)).toBe(3);
    expect(slotFromY(-80, 50, 4)).toBe(0);
    expect(slotFromY(9999, 50, 4)).toBe(4);
    expect(slotFromY(10, 50, 0)).toBeNull();
  });

  it("the slots right above and below the dragged row change nothing", () => {
    expect(isNoopSlot(2, 2)).toBe(true);
    expect(isNoopSlot(2, 3)).toBe(true);
    expect(isNoopSlot(2, 1)).toBe(false);
    expect(isNoopSlot(2, 4)).toBe(false);
  });

  it("turns a slot into the index the row ends up at", () => {
    // [a b c d e], dragging c (2)
    expect(targetIndex(2, 0)).toBe(0); // above a
    expect(targetIndex(2, 5)).toBe(4); // below e
    expect(targetIndex(2, 4)).toBe(3); // between d and e
    expect(targetIndex(0, 2)).toBe(1); // a between b and c
  });

  it("the two rows beside the slot step apart", () => {
    expect([0, 1, 2, 3].map((i) => partingFor(i, 2))).toEqual([0, -1, 1, 0]);
    expect([0, 1].map((i) => partingFor(i, 0))).toEqual([1, 0]);
    expect([0, 1].map((i) => partingFor(i, null))).toEqual([0, 0]);
  });
});
