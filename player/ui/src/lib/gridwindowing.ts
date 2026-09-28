/**
 * Pure layout math for the virtualized album grid. No DOM, no Vue — every
 * function here is unit-testable in plain Node.
 *
 * Same pattern as Koa's photo grid: a fixed-lanes layout (square-ish cells,
 * `lanes` columns, one virtual row per `lanes` items). Row height is
 * deterministic from the container width and the minimum cell width, so the
 * virtualizer never needs per-item dynamic measurement — a resize just
 * recomputes this layout and the virtualizer re-measures.
 *
 * Note: the virtualizer itself runs with `lanes: 1` (one virtual item = one
 * full-width row strip). Its own `lanes` option is a masonry mode — one
 * measurement per item — which is a different layout; the lane math below
 * maps row strips to items instead.
 */

export interface GridLayout {
  /** Columns per row. */
  lanes: number;
  /** Cell edge in px (grid is `auto-fill`-equivalent, cells fill the row). */
  cellWidth: number;
  /** Virtual row height in px (cell + gap). */
  rowHeight: number;
  /** Number of virtual rows for the item count. */
  rowCount: number;
}

/** Columns that fit in `containerWidth` given a minimum cell width. */
export function computeLanes(containerWidth: number, minCellWidth: number, gap: number): number {
  if (containerWidth <= 0 || minCellWidth <= 0) return 1;
  return Math.max(1, Math.floor((containerWidth + gap) / (minCellWidth + gap)));
}

/**
 * Full layout for `itemCount` items at the given geometry. `extraContentHeight`
 * is added to each row's height on top of the square cell — for a card that
 * renders anything below its (square) artwork, e.g. a title/subtitle block,
 * so the virtualizer's fixed row height actually fits the whole card and
 * doesn't clip or overlap the row below it.
 */
export function computeGridLayout(
  containerWidth: number,
  itemCount: number,
  minCellWidth: number,
  gap: number,
  extraContentHeight = 0,
): GridLayout {
  const lanes = computeLanes(containerWidth, minCellWidth, gap);
  const cellWidth = containerWidth > 0 ? (containerWidth - (lanes - 1) * gap) / lanes : 0;
  return {
    lanes,
    cellWidth,
    rowHeight: cellWidth + extraContentHeight + gap,
    rowCount: Math.ceil(itemCount / lanes),
  };
}

/** Item ids rendered by virtual row `rowIndex`. */
export function rowItemIds(rowIndex: number, lanes: number, ids: number[]): number[] {
  const start = rowIndex * lanes;
  return ids.slice(start, start + lanes);
}

/**
 * How many DOM rows the virtualizer may realize for the given viewport.
 * Used by the perf smoke test to assert the DOM stays bounded as the
 * library grows: realized rows = visible rows + 2 × overscan, regardless of
 * total item count. The +1 covers a mid-row scroll offset, where both
 * partially-visible edge rows are realized.
 */
export function maxRealizedRows(viewportHeight: number, rowHeight: number, overscan: number): number {
  if (rowHeight <= 0) return 0;
  return Math.ceil(viewportHeight / rowHeight) + 1 + 2 * overscan;
}
