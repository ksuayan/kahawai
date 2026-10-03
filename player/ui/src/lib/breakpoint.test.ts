import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { defineComponent, h } from "vue";
import { mount } from "@vue/test-utils";
import { PHONE_MAX_WIDTH, useBreakpoint } from "./breakpoint";

function makeMatchMedia(matches: boolean) {
  const listeners = new Set<() => void>();
  const mq = {
    matches,
    addEventListener: vi.fn((_t: string, fn: () => void) => listeners.add(fn)),
    removeEventListener: vi.fn((_t: string, fn: () => void) => listeners.delete(fn)),
    __fire: (m: boolean) => {
      (mq as { matches: boolean }).matches = m;
      listeners.forEach((fn) => fn());
    },
  };
  return mq;
}

describe("useBreakpoint", () => {
  let mq: ReturnType<typeof makeMatchMedia>;
  const realMatchMedia = window.matchMedia;

  beforeEach(() => {
    mq = makeMatchMedia(false);
    window.matchMedia = vi.fn().mockReturnValue(mq) as unknown as typeof window.matchMedia;
  });

  afterEach(() => {
    window.matchMedia = realMatchMedia;
    vi.restoreAllMocks();
  });

  it("queries the phone max width", () => {
    const Probe = defineComponent({
      setup() {
        const { isPhone } = useBreakpoint();
        return () => h("div", String(isPhone.value));
      },
    });
    mount(Probe);
    expect(window.matchMedia).toHaveBeenCalledWith(`(max-width: ${PHONE_MAX_WIDTH}px)`);
  });

  it("reflects the media query and updates on change", async () => {
    let seen = "";
    const Probe = defineComponent({
      setup() {
        const { isPhone } = useBreakpoint();
        return () => {
          seen = String(isPhone.value);
          return h("div", seen);
        };
      },
    });
    const w = mount(Probe);
    expect(seen).toBe("false");
    mq.__fire(true);
    await w.vm.$nextTick();
    expect(seen).toBe("true");
  });

  it("removes the listener on unmount", () => {
    const Probe = defineComponent({
      setup() {
        useBreakpoint();
        return () => h("div");
      },
    });
    const w = mount(Probe);
    w.unmount();
    expect(mq.removeEventListener).toHaveBeenCalledTimes(1);
  });
});
