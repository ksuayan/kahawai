import { describe, expect, it } from "vitest";
import { mountApp, settle } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import PhoneTabBar from "./PhoneTabBar.vue";

describe("PhoneTabBar", () => {
  it("renders seven icon-only tabs", async () => {
    const { wrapper } = mountApp(PhoneTabBar);
    await settle();
    const tabs = wrapper.findAll('[data-testid^="tab-"]');
    expect(tabs).toHaveLength(7);
    expect(tabs.map((t) => t.attributes("data-testid"))).toEqual([
      "tab-library",
      "tab-audiobooks",
      "tab-podcasts",
      "tab-radio",
      "tab-search",
      "tab-queue",
      "tab-settings",
    ]);
    // Icon-only: no visible text labels; every tab names itself for AT.
    for (const t of tabs) {
      expect(t.text().trim()).toBe("");
      expect(t.attributes("aria-label")).toBeTruthy();
    }
  });

  it("navigates to podcasts and radio", async () => {
    const { wrapper } = mountApp(PhoneTabBar);
    await settle();
    await wrapper.find('[data-testid="tab-podcasts"]').trigger("click");
    expect(useNavStore().section).toBe("podcasts");
    await wrapper.find('[data-testid="tab-radio"]').trigger("click");
    expect(useNavStore().section).toBe("radio");
  });

  it("marks the active tab with aria-current", async () => {
    const { wrapper } = mountApp(PhoneTabBar);
    await settle();
    // Default nav section is albums → Library tab active.
    const library = wrapper.find('[data-testid="tab-library"]');
    expect(library.attributes("aria-current")).toBe("page");
    const settings = wrapper.find('[data-testid="tab-settings"]');
    expect(settings.attributes("aria-current")).toBeUndefined();
  });

  it("colours only the active tab with the accent (not both colour classes at once)", async () => {
    const { wrapper } = mountApp(PhoneTabBar);
    useNavStore().go("settings");
    await settle();
    const settings = wrapper.get('[data-testid="tab-settings"]').classes();
    const search = wrapper.get('[data-testid="tab-search"]').classes();
    expect(settings).toContain("text-accent");
    expect(settings).not.toContain("text-dim");
    expect(search).toContain("text-dim");
    expect(search).not.toContain("text-accent");
  });

  it("has a labeled navigation landmark", async () => {
    const { wrapper } = mountApp(PhoneTabBar);
    await settle();
    expect(wrapper.find('nav[aria-label="Primary"]').exists()).toBe(true);
  });
});
