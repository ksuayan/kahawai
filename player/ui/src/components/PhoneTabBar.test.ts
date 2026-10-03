import { describe, expect, it } from "vitest";
import { mountApp, settle } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import PhoneTabBar from "./PhoneTabBar.vue";

describe("PhoneTabBar", () => {
  it("renders five tabs", async () => {
    const { wrapper } = mountApp(PhoneTabBar);
    await settle();
    const tabs = wrapper.findAll('[data-testid^="tab-"]');
    expect(tabs).toHaveLength(5);
    expect(tabs.map((t) => t.attributes("data-testid"))).toEqual([
      "tab-library",
      "tab-audiobooks",
      "tab-search",
      "tab-queue",
      "tab-settings",
    ]);
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
