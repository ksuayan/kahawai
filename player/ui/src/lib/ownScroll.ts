import { nextTick, onBeforeUnmount, watch, type Ref } from "vue";
import { useScrollMemoryStore } from "../stores/scrollMemory";

/**
 * Scroll memory for a view with its own scroll container (the virtualized
 * lists and grids): put the scrollbar back when the container appears, and
 * remember where it was when the view goes away. The container may appear
 * after mount (a view that was still loading), hence the watch.
 */
export function useOwnScrollMemory(el: Ref<HTMLElement | null>, key: () => string): void {
  const memory = useScrollMemoryStore();
  watch(
    el,
    (node) => {
      if (!node) return;
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
    if (el.value) memory.set(key(), el.value.scrollTop);
  });
}
