import { onMounted, onUnmounted, type Ref } from "vue";

/**
 * Long-press detection for touch: press-and-hold without much movement
 * fires the callback, which typically opens a context menu. Movement beyond
 * the slop or early release cancels. Mouse right-click is unaffected (the
 * browser fires `contextmenu` itself).
 *
 * The callback receives the originating PointerEvent so callers can
 * synthesize a `contextmenu` MouseEvent at the touch point — Reka's
 * ContextMenuTrigger opens on those with no further changes.
 */
export function useLongPress(
  el: Ref<HTMLElement | null>,
  onLongPress: (e: PointerEvent) => void,
  delayMs = 500,
): void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let startX = 0;
  let startY = 0;
  const SLOP = 10;

  const cancel = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  const down = (e: PointerEvent) => {
    if (e.pointerType === "mouse") return;
    startX = e.clientX;
    startY = e.clientY;
    cancel();
    timer = setTimeout(() => {
      timer = null;
      onLongPress(e);
    }, delayMs);
  };

  const move = (e: PointerEvent) => {
    if (timer === null) return;
    if (Math.hypot(e.clientX - startX, e.clientY - startY) > SLOP) cancel();
  };

  let target: HTMLElement | null = null;
  const attach = () => {
    target = el.value;
    target?.addEventListener("pointerdown", down);
    target?.addEventListener("pointermove", move);
    target?.addEventListener("pointerup", cancel);
    target?.addEventListener("pointercancel", cancel);
  };
  const detach = () => {
    cancel();
    target?.removeEventListener("pointerdown", down);
    target?.removeEventListener("pointermove", move);
    target?.removeEventListener("pointerup", cancel);
    target?.removeEventListener("pointercancel", cancel);
    target = null;
  };

  // The ref is set by the time onMounted runs.
  onMounted(attach);
  onUnmounted(detach);
}

/** Open a Reka context menu at the touch point, as if right-clicked there. */
export function fireContextMenu(e: PointerEvent): void {
  const target = e.target as HTMLElement | null;
  target?.dispatchEvent(
    new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: e.clientX,
      clientY: e.clientY,
    }),
  );
}
