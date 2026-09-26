import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { mountApp, settle } from "../test/helpers";
import Sidebar from "../components/Sidebar.vue";
import { useThemeStore } from "./theme";

beforeEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.theme;
  vi.restoreAllMocks();
});

const prefersLight = (light: boolean) =>
  vi.spyOn(window, "matchMedia").mockImplementation(((q: string) => ({ matches: light && q.includes("light"), media: q, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, onchange: null, dispatchEvent: () => false })) as never);

describe("theme store", () => {
  it("is dark by default and applies data-theme to <html>", () => {
    prefersLight(false);
    setActivePinia(createPinia());
    const t = useThemeStore();
    t.init();
    expect(t.theme).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("follows the system light preference until the user chooses", () => {
    prefersLight(true);
    setActivePinia(createPinia());
    expect(useThemeStore().theme).toBe("light");
  });

  it("toggle flips, applies and remembers the choice over the system setting", () => {
    prefersLight(true);
    setActivePinia(createPinia());
    const t = useThemeStore();
    t.toggle();
    expect(t.theme).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(localStorage.getItem("kahawai-player.theme")).toBe("dark");
    setActivePinia(createPinia());
    expect(useThemeStore().theme).toBe("dark");
  });
});

describe("sidebar theme toggle", () => {
  it("sits to the right of Settings with an accessible label and switches the theme", async () => {
    prefersLight(false);
    const { wrapper } = mountApp(Sidebar);
    const buttons = wrapper.findAll("button");
    const settings = buttons.findIndex((b) => b.text().includes("Settings"));
    const toggle = wrapper.get('[data-testid="theme-toggle"]');
    expect(buttons.indexOf(toggle as never) === -1 ? buttons.findIndex((b) => b.element === toggle.element) : 0).toBe(settings + 1);
    expect(toggle.attributes("aria-label")).toBe("Switch to light theme");
    await toggle.trigger("click");
    await settle();
    expect(useThemeStore().theme).toBe("light");
    expect(toggle.attributes("aria-label")).toBe("Switch to dark theme");
    expect(document.documentElement.dataset.theme).toBe("light");
  });
});
