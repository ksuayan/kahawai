/**
 * Pure geometry for drag-reordering a list of fixed-height rows (the Queue).
 *
 * Rows are `rowHeight` px tall and stacked from the top of the rows container,
 * so the insertion slot under the pointer follows from the pointer's offset
 * alone: no DOM measurement, and it keeps working for virtualized rows.
 *
 * A *slot* is a boundary between rows: 0 is above the first row, n is below
 * the last. The dragged row lands before the row currently at the slot.
 */

/** The slot nearest to `y` (px from the top of the rows container). */
export function slotFromY(y: number, rowHeight: number, n: number): number | null {
  if (n <= 0 || rowHeight <= 0) return null;
  return Math.min(n, Math.max(0, Math.round(y / rowHeight)));
}

/** Dropping row `from` at `slot` leaves the order as it is (the slots just above and below it). */
export function isNoopSlot(from: number, slot: number): boolean {
  return slot === from || slot === from + 1;
}

/** The index `from` ends up at when dropped at `slot` (the list is one shorter once it is lifted). */
export function targetIndex(from: number, slot: number): number {
  return slot > from ? slot - 1 : slot;
}

/** Which way row `i` steps aside to open the slot: -1 (up) for the row above it, +1 (down) below, else 0. */
export function partingFor(i: number, slot: number | null): -1 | 0 | 1 {
  if (slot === null) return 0;
  if (i === slot - 1) return -1;
  if (i === slot) return 1;
  return 0;
}
