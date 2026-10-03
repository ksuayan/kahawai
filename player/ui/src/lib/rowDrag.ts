import { onBeforeUnmount, ref } from "vue";
import { isNoopSlot, partingFor, slotFromY, targetIndex } from "./listdrop";

/** Pixels the pointer must travel before a press becomes a drag (so clicks still click). */
const DRAG_THRESHOLD = 5;
/** How far the two rows beside the open slot step apart (px). */
export const ROW_PART = 4;
const EDGE = 48;

export interface RowDragOptions {
  /** Row height of the list, px: the drop geometry is computed from it. */
  rowHeight: number;
  /** Rearranging is allowed now (off while sorted for viewing, or busy). */
  enabled: () => boolean;
  /** How many rows there are. */
  count: () => number;
  /** Move the row at `from` so it ends up at `to`. */
  onMove: (from: number, to: number) => unknown;
}

/**
 * Drag-to-reorder for a virtualized list of fixed-height rows, pointer-based
 * like Koa's photo grid, NOT the browser's native drag-and-drop: in the webview
 * the native drag image of a transformed row is a faint copy of the wrong row
 * sliding in from the top, and it can't be styled. Here the pointer drives it
 * all: a copy of the row follows the pointer, the row stays put (dimmed, in a
 * dashed outline drawn by RowDragOverlay), and the gap where it will land opens
 * and is marked. Esc cancels; near the edges the list scrolls. The list must be
 * a VirtualList (it finds `[data-rows]` and `[data-scroller]` around the row).
 */
export function useRowDrag(o: RowDragOptions) {
  /** The row being dragged. */
  const dragFrom = ref<number | null>(null);
  /** The insertion slot under the pointer (0..n), when dropping there would change the order. */
  const slot = ref<number | null>(null);
  let armed: { i: number; x: number; y: number; row: HTMLElement } | null = null;
  let ghost: HTMLElement | null = null;
  let grab = { x: 0, y: 0 };
  let pointer = { x: 0, y: 0 };
  let rowsEl: HTMLElement | null = null;
  let scrollEl: HTMLElement | null = null;

  function onRowPointerDown(e: PointerEvent, i: number): void {
    if (!o.enabled() || e.button !== 0) return;
    if ((e.target as HTMLElement | null)?.closest("button")) return; // the row's own buttons
    // Touch: only the grip handle (`data-drag-handle`) starts a drag, so a swipe
    // anywhere else still scrolls the list.
    if (e.pointerType === "touch" && !(e.target as HTMLElement | null)?.closest("[data-drag-handle]")) return;
    armed = { i, x: e.clientX, y: e.clientY, row: e.currentTarget as HTMLElement };
    pointer = { x: e.clientX, y: e.clientY };
    window.addEventListener("pointermove", onPointerMove, true);
    window.addEventListener("pointerup", onPointerUp, true);
    window.addEventListener("pointercancel", finishDrag, true);
    window.addEventListener("keydown", onDragKey, true);
  }

  function detach(): void {
    window.removeEventListener("pointermove", onPointerMove, true);
    window.removeEventListener("pointerup", onPointerUp, true);
    window.removeEventListener("pointercancel", finishDrag, true);
    window.removeEventListener("keydown", onDragKey, true);
  }

  function onPointerMove(e: PointerEvent): void {
    pointer = { x: e.clientX, y: e.clientY };
    if (dragFrom.value === null) {
      if (!armed || Math.hypot(e.clientX - armed.x, e.clientY - armed.y) < DRAG_THRESHOLD) return;
      beginDrag();
    }
    moveGhost();
    updateSlot();
    autoScrollFor(e.clientY);
    e.preventDefault();
  }

  function beginDrag(): void {
    if (!armed) return;
    dragFrom.value = armed.i;
    slot.value = null;
    rowsEl = armed.row.closest<HTMLElement>("[data-rows]");
    scrollEl = armed.row.closest<HTMLElement>("[data-scroller]");
    makeGhost(armed.row);
    document.body.style.cursor = "grabbing";
    document.body.style.userSelect = "none";
    window.getSelection()?.removeAllRanges();
  }

  /** A copy of the grabbed row that follows the pointer. */
  function makeGhost(row: HTMLElement): void {
    const rect = row.getBoundingClientRect();
    const copy = row.cloneNode(true) as HTMLElement;
    copy.removeAttribute("title");
    copy.setAttribute("data-testid", "drag-ghost");
    copy.style.cssText +=
      ";position:fixed;left:0;top:0;margin:0;z-index:80;pointer-events:none;opacity:.94;" +
      "background:var(--color-surface);box-shadow:var(--shadow-float);border-radius:8px;" +
      `width:${rect.width}px;height:${rect.height}px;will-change:transform;`;
    grab = { x: pointer.x - rect.left, y: pointer.y - rect.top };
    document.body.appendChild(copy);
    ghost = copy;
  }

  function moveGhost(): void {
    if (ghost) ghost.style.transform = `translate(${pointer.x - grab.x}px, ${pointer.y - grab.y}px) scale(1.02)`;
  }

  /** The slot under the pointer, if dropping there would move the row; none outside the list. */
  function updateSlot(): void {
    const from = dragFrom.value;
    const box = scrollEl?.getBoundingClientRect();
    const rows = rowsEl?.getBoundingClientRect();
    if (from === null || !box || !rows) return;
    const { x, y } = pointer;
    if (x < box.left || x > box.right || y < box.top || y > box.bottom) {
      slot.value = null;
      return;
    }
    const s = slotFromY(y - rows.top, o.rowHeight, o.count());
    slot.value = s === null || isNoopSlot(from, s) ? null : s;
  }

  async function onPointerUp(e: PointerEvent): Promise<void> {
    pointer = { x: e.clientX, y: e.clientY };
    const from = dragFrom.value;
    if (from === null) {
      detach(); // a plain click: leave it to the row
      armed = null;
      return;
    }
    updateSlot(); // where it was let go
    const s = slot.value;
    finishDrag();
    swallowNextClick(); // the release must not also click the row under it
    if (s !== null) await o.onMove(from, targetIndex(from, s));
  }

  /** Escape while holding a row cancels: nothing moves. */
  function onDragKey(e: KeyboardEvent): void {
    if (e.key !== "Escape") return;
    if (dragFrom.value !== null) {
      e.stopPropagation();
      e.preventDefault();
    }
    finishDrag();
  }

  function finishDrag(): void {
    detach();
    armed = null;
    dragFrom.value = null;
    slot.value = null;
    stopAutoScroll();
    ghost?.remove();
    ghost = null;
    rowsEl = scrollEl = null;
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }

  function swallowNextClick(): void {
    const h = (ev: MouseEvent) => {
      ev.stopPropagation();
      ev.preventDefault();
    };
    window.addEventListener("click", h, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", h, true), 0);
  }

  // Edge auto-scroll: dragging near the top or bottom of a long list scrolls it.
  let scrollTimer: ReturnType<typeof setInterval> | null = null;
  let scrollVelocity = 0;

  function autoScrollFor(clientY: number): void {
    const r = scrollEl?.getBoundingClientRect();
    if (!r) return;
    if (clientY < r.top + EDGE) scrollVelocity = -Math.ceil(((r.top + EDGE - clientY) / EDGE) * 16);
    else if (clientY > r.bottom - EDGE) scrollVelocity = Math.ceil(((clientY - (r.bottom - EDGE)) / EDGE) * 16);
    else scrollVelocity = 0;
    if (scrollVelocity !== 0 && !scrollTimer) {
      scrollTimer = setInterval(() => {
        if (scrollVelocity === 0) return stopAutoScroll();
        scrollEl?.scrollBy(0, scrollVelocity);
        updateSlot(); // the list moved under a still pointer
      }, 16);
    } else if (scrollVelocity === 0) {
      stopAutoScroll();
    }
  }

  function stopAutoScroll(): void {
    if (scrollTimer) clearInterval(scrollTimer);
    scrollTimer = null;
    scrollVelocity = 0;
  }

  onBeforeUnmount(finishDrag);

  /** The two rows beside the open slot step apart to show the gap. */
  function rowShift(i: number): string | undefined {
    const side = partingFor(i, slot.value);
    return side ? `translateY(${side * ROW_PART}px)` : undefined;
  }

  return { dragFrom, slot, onRowPointerDown, rowShift };
}
