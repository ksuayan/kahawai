import { inject, nextTick, onBeforeUnmount, watch, type InjectionKey, type Ref } from "vue";
import { useScrollMemoryStore } from "../stores/scrollMemory";
import { saveWhileScrolling } from "./ownScroll";

/** The app's shared scrolling `<main>` (provided by App.vue). Most views scroll
 *  inside it rather than in a container of their own. */
export const MAIN_SCROLL: InjectionKey<Ref<HTMLElement | null>> = Symbol("mainScroll");

/**
 * Remember and restore a view's place in the shared `<main>` scroller.
 *
 * Views unmount when the user navigates away, and `<main>` keeps whatever
 * offset it last had, so without this a returning view lands on a stale or
 * zero offset. `ready` says when the view's content is actually in the DOM
 * (not still loading); the restore waits for it, because the browser clamps
 * `scrollTop` to the content height. A view left before it was ready keeps
 * the offset it already had. No-op when there is no shared scroller (tests).
 */
export function useMainScrollMemory(key: string, ready: () => boolean): void {
  const scroller = inject(MAIN_SCROLL, null);
  if (!scroller) return;
  const memory = useScrollMemoryStore();
  let restored = false;
  let stopSaving: (() => void) | undefined;

  watch(
    ready,
    (isReady) => {
      if (!isReady || restored) return;
      void nextTick(() => {
        const el = scroller.value;
        if (!el || restored) return;
        el.scrollTop = memory.get(key); // 0 when never scrolled: also drops a stale offset
        restored = true;
        stopSaving = saveWhileScrolling(el, () => memory.set(key, el.scrollTop));
      });
    },
    { immediate: true },
  );

  onBeforeUnmount(() => {
    stopSaving?.();
    const el = scroller.value;
    if (el && restored) memory.set(key, el.scrollTop);
  });
}
