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
});
