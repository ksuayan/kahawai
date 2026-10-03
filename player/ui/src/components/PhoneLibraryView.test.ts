import { describe, expect, it, vi } from "vitest";
import { mountApp, settle } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import PhoneLibraryView from "./PhoneLibraryView.vue";

vi.mock("../lib/breakpoint", () => ({ useBreakpoint: () => ({ isPhone: { value: true } }) }));

describe("PhoneLibraryView", () => {
  it("renders the four library segments", async () => {
    const { wrapper } = mountApp(PhoneLibraryView);
    await settle();
    for (const name of ["albums", "artists", "genres", "playlists"]) {
      expect(wrapper.find(`[data-testid="lib-tab-${name}"]`).exists()).toBe(true);
    }
  });

  it("switches sections through the segmented control", async () => {
    const { wrapper } = mountApp(PhoneLibraryView, {}, {}, () => {
      useNavStore().go("albums");
    });
    await settle();
    expect(wrapper.find('[data-testid="lib-tab-albums"]').attributes("aria-selected")).toBe("true");
    await wrapper.find('[data-testid="lib-tab-artists"]').trigger("click");
    await settle();
    expect(useNavStore().section).toBe("artists");
    expect(wrapper.find('[data-testid="lib-tab-artists"]').attributes("aria-selected")).toBe("true");
  });

  it("gives a virtualized section a bounded box (so it renders only what is on screen) and lets the others scroll", async () => {
    const { wrapper } = mountApp(PhoneLibraryView, {}, {}, () => useNavStore().go("albums"));
    await settle();
    const box = () => wrapper.get('[data-testid="phone-library-box"]').classes();
    expect(box()).toEqual(expect.arrayContaining(["flex", "flex-col", "overflow-hidden", "min-h-0", "flex-1"]));
    useNavStore().go("playlists");
    await settle();
    expect(box()).toContain("overflow-y-auto");
    expect(box()).not.toContain("overflow-hidden");
  });
});

describe("PhoneShell", () => {
  it("bounds the library and other virtualized screens, and scrolls the rest", async () => {
    const { default: PhoneShell } = await import("./PhoneShell.vue");
    const { wrapper } = mountApp(PhoneShell, {}, {}, () => useNavStore().go("queue"));
    await settle();
    const box = () => wrapper.get('[data-testid="phone-view-box"]').classes();
    expect(box()).toEqual(expect.arrayContaining(["flex", "flex-col", "overflow-hidden"]));
    useNavStore().go("albums");
    await settle();
    expect(box()).toEqual(expect.arrayContaining(["flex", "flex-col", "overflow-hidden"]));
    useNavStore().go("settings");
    await settle();
    expect(box()).toContain("overflow-y-auto");
  });
});
