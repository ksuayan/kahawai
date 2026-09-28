/**
 * Smoke test: a huge library must keep the realized DOM bounded.
 *
 * Same technique as Koa's photo grid smoke test: mounts the real
 * @tanstack/vue-virtual virtualizer with the same configuration AlbumsView
 * uses (one virtual item = one row strip, overscan 3), feeding it a fake
 * viewport/scroll via `observeElementRect`/`observeElementOffset` instead of
 * relying on real browser layout (which happy-dom doesn't have). Asserts the
 * number of realized rows depends only on viewport + overscan — never on
 * library size. AlbumsView renders one AlbumCard per (row × lane), so the
 * DOM cell bound is realizedRows × lanes.
 */
import { describe, expect, it } from "vitest";
import { createApp, h, nextTick, type Ref } from "vue";
import { useVirtualizer, type Virtualizer } from "@tanstack/vue-virtual";
import { computeGridLayout, maxRealizedRows } from "./gridwindowing";

const GAP = 16;
const OVERSCAN = 3;
const VIEWPORT = { width: 1200, height: 900 };
// 100k albums at the AlbumCard minimum width.
const LAYOUT = computeGridLayout(VIEWPORT.width, 100_000, 160, GAP);
const ROW_COUNT = LAYOUT.rowCount;

function makeVirtualizer() {
  // Box object: property narrowing survives closure capture, unlike `let`.
  const box: {
    v?: Ref<Virtualizer<HTMLElement, HTMLElement>>;
    offsetCb?: (offset: number, isScrolling: boolean) => void;
  } = {};
  const host = document.createElement("div");
  document.body.appendChild(host);
  const app = createApp({
    setup() {
      box.v = useVirtualizer({
        count: ROW_COUNT,
        getScrollElement: (): HTMLElement | null => host,
        estimateSize: () => LAYOUT.rowHeight,
        lanes: 1,
        overscan: OVERSCAN,
        observeElementRect: (_instance, cb) => {
          cb(VIEWPORT);
        },
        observeElementOffset: (_instance, cb) => {
          box.offsetCb = cb;
          cb(0, false);
        },
      });
      return () => h("div");
    },
  });
  app.mount(host);
  if (!box.v || !box.offsetCb) throw new Error("virtualizer did not initialize");
  const v = box.v;
  const offsetCb = box.offsetCb;
  return { v, offsetCb, app, host };
}

const ROW_BOUND = maxRealizedRows(VIEWPORT.height, LAYOUT.rowHeight, OVERSCAN);

describe("100k-album virtualized grid", () => {
  it("realizes a bounded row window at scroll top", async () => {
    const { v, app, host } = makeVirtualizer();
    await nextTick();
    const items = v.value.getVirtualItems();
    expect(items.length).toBeGreaterThan(0);
    expect(items.length).toBeLessThanOrEqual(ROW_BOUND);
    // Window starts at the top: contiguous rows from 0.
    expect(items[0].index).toBe(0);
    items.forEach((it, i) => expect(it.index).toBe(i));
    // Full scroll height still accounts for every row (fractional
    // cellWidth summed across 16k+ rows accumulates a little float drift,
    // hence toBeCloseTo rather than exact equality).
    expect(v.value.getTotalSize()).toBeCloseTo(ROW_COUNT * LAYOUT.rowHeight, 0);
    // DOM bound for AlbumsView: rows × lanes cells — independent of 100k.
    expect(items.length * LAYOUT.lanes).toBeLessThanOrEqual(ROW_BOUND * LAYOUT.lanes);
    app.unmount();
    host.remove();
  });

  it("moves the window on scroll without growing it", async () => {
    const { v, offsetCb, app, host } = makeVirtualizer();
    await nextTick();
    const scrollPx = 500_000;
    offsetCb(scrollPx, true);
    await nextTick();
    const items = v.value.getVirtualItems();
    expect(items.length).toBeLessThanOrEqual(ROW_BOUND);
    // Window sits around the scrolled position, not at 0.
    const firstRow = Math.floor(scrollPx / LAYOUT.rowHeight);
    expect(items[0].index).toBeLessThanOrEqual(firstRow);
    expect(items[items.length - 1].index).toBeGreaterThanOrEqual(firstRow);
    app.unmount();
    host.remove();
  });
});
