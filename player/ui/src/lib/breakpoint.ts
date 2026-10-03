import { onMounted, onUnmounted, ref, type Ref } from "vue";

/** Width at or below which the phone layout takes over. */
export const PHONE_MAX_WIDTH = 719;

/**
 * Reactive phone-layout flag. Width-based (not user-agent): narrow desktop
 * windows and devtools get the phone layout too, which makes it testable
 * without a device. SSR-safe: defaults to desktop (false) until mounted.
 */
export function useBreakpoint(): { isPhone: Ref<boolean> } {
  const isPhone = ref(false);
  let mq: MediaQueryList | null = null;

  const update = () => {
    isPhone.value = mq?.matches ?? false;
  };

  onMounted(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    mq = window.matchMedia(`(max-width: ${PHONE_MAX_WIDTH}px)`);
    update();
    mq.addEventListener("change", update);
  });

  onUnmounted(() => {
    mq?.removeEventListener("change", update);
    mq = null;
  });

  return { isPhone };
}
