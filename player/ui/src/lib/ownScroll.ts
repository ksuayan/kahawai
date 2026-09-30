import { nextTick, onBeforeUnmount, watch, type Ref } from "vue";
import { useScrollMemoryStore } from "../stores/scrollMemory";

/** While scrolling, the position is saved at most this often (it's also
 *  saved on leaving the view). The app may quit without leaving it. */
export const SCROLL_SAVE_MS = 400;

/** Calls `save` at most every SCROLL_SAVE_MS while `el` scrolls. Returns a
 *  function that stops listening. */
export function saveWhileScrolling(el: HTMLElement, save: () => void): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const onScroll = () => {
    if (timer !== undefined) return;
    timer = setTimeout(() => {
      timer = undefined;
      save();
    }, SCROLL_SAVE_MS);
  };
  el.addEventListener("scroll", onScroll, { passive: true });
  return () => {
    clearTimeout(timer);
    el.removeEventListener("scroll", onScroll);
  };
}

/**
 * Scroll memory for a view with its own scroll container (the virtualized
 * lists and grids): put the scrollbar back when the container appears, and
 * remember where it was when the view goes away. The container may appear
 * after mount (a view that was still loading), hence the watch.
 */
export function useOwnScrollMemory(el: Ref<HTMLElement | null>, key: () => string): void {
  const memory = useScrollMemoryStore();
  let stopSaving: (() => void) | undefined;
  watch(
    el,
    (node) => {
      stopSaving?.();
      stopSaving = undefined;
      if (!node) return;
      stopSaving = saveWhileScrolling(node, () => memory.set(key(), node.scrollTop));
      // The browser clamps scrollTop to the content height, so wait for the
      // sized inner strip to be in the DOM.
      void nextTick(() => {
        const saved = memory.get(key());
        if (saved > 0 && el.value === node) node.scrollTop = saved;
      });
    },
    { immediate: true },
  );
  onBeforeUnmount(() => {
    stopSaving?.();
    if (el.value) memory.set(key(), el.value.scrollTop);
  });
}
