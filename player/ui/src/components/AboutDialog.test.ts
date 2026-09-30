import { describe, expect, it } from "vitest";
import { $$, mountApp, settle } from "../test/helpers";
import { useOverlaysStore } from "../stores/overlays";
import AboutDialog from "./AboutDialog.vue";
import Sidebar from "./Sidebar.vue";

async function boot() {
  const { wrapper } = mountApp(AboutDialog);
  await settle();
  return { wrapper, overlays: useOverlaysStore() };
}
const dialog = () => document.body.querySelector('[data-testid="about-dialog"]');
const content = () => document.body.querySelector('[data-testid="about-content"]') as HTMLElement;

describe("AboutDialog", () => {
  it("is closed until opened, then shows the About page with the version", async () => {
    const { overlays } = await boot();
    expect(dialog()).toBeNull();
    overlays.openAbout();
    await settle();
    expect(dialog()).not.toBeNull();
    expect(document.body.querySelector('[role="dialog"]')).not.toBeNull();
    expect(content().querySelector("h1")!.textContent).toBe("Kahawai Player");
    expect(content().textContent).toMatch(/Version \d+\.\d+\.\d+/); // taken from tauri.conf.json, not "{{version}}"
    expect(content().textContent).not.toContain("{{version}}");
  });

  it("shows the splash artwork above the title on the About page, not on the notices", async () => {
    const { overlays } = await boot();
    overlays.openAbout();
    await settle();
    const img = document.body.querySelector('[data-testid="about-image"]') as HTMLImageElement;
    expect(img.getAttribute("src")).toBe("/splash.webp");
    // Before the content (and so its title) in the document.
    expect(img.compareDocumentPosition(content()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    (document.body.querySelector('[data-testid="about-tab-notices"]') as HTMLElement).click();
    await settle();
    expect(document.body.querySelector('[data-testid="about-image"]')).toBeNull();
  });

  it("states the license (no warranty, where the source is) and shows its full text", async () => {
    const { overlays } = await boot();
    overlays.openAbout();
    await settle();
    const text = content().textContent!;
    expect(text).toContain("GNU Affero General Public License");
    expect(text).toContain("without any warranty");
    expect(text).toContain("https://github.com/ksuayan/kahawai");
    expect(text).not.toContain("All rights reserved");
    (document.body.querySelector('[data-testid="about-tab-license"]') as HTMLElement).click();
    await settle();
    const license = document.body.querySelector('[data-testid="about-license"]')!.textContent!;
    expect(license).toContain("GNU AFFERO GENERAL PUBLIC LICENSE");
    expect(license).toContain("Version 3, 19 November 2007");
  });

  it("ends the About page with the disclaimers", async () => {
    const { overlays } = await boot();
    overlays.openAbout();
    await settle();
    const headings = [...content().querySelectorAll("h2")].map((h) => h.textContent);
    expect(headings.at(-1)).toBe("Disclaimers");
    expect(content().textContent).toContain("No warranty.");
    expect(content().textContent).toContain("Your hearing and your equipment.");
  });

  it("has accessible tabs, switches to the notices, and reopens on About", async () => {
    const { overlays } = await boot();
    overlays.openAbout();
    await settle();
    const tabs = $$('[role="tab"]');
    expect(tabs.map((t) => t.textContent?.trim())).toEqual(["About", "Open-source notices", "License"]);
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
    (document.body.querySelector('[data-testid="about-tab-notices"]') as HTMLElement).click();
    await settle();
    expect(content().querySelector("h1")!.textContent).toBe("Open-source notices");
    expect(content().textContent).toContain("SIL OPEN FONT LICENSE Version 1.1");
    expect(document.body.querySelector('[data-testid="about-tab-notices"]')!.getAttribute("aria-selected")).toBe("true");
    overlays.aboutOpen = false;
    await settle();
    overlays.openAbout();
    await settle();
    expect(content().querySelector("h1")!.textContent).toBe("Kahawai Player");
  });

  it("closes from the close button and from Escape, and names itself for screen readers", async () => {
    const { overlays } = await boot();
    overlays.openAbout();
    await settle();
    expect(document.body.querySelector('[role="dialog"]')!.textContent).toContain("About Kahawai Player");
    (document.body.querySelector('button[aria-label="Close"]') as HTMLElement).click();
    await settle();
    expect(overlays.aboutOpen).toBe(false);
    overlays.openAbout();
    await settle();
    document.body.querySelector('[role="dialog"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    expect(overlays.aboutOpen).toBe(false);
  });

  it("never renders a live link (the window may not open external addresses)", async () => {
    const { overlays } = await boot();
    overlays.openAbout();
    await settle();
    expect(content().querySelector("a")).toBeNull();
  });
});

describe("About button in the sidebar", () => {
  it("has a labelled icon button that opens the dialog", async () => {
    const { wrapper } = mountApp(Sidebar);
    const btn = wrapper.get('[data-testid="about-button"]');
    expect(btn.attributes("aria-label")).toBe("About Kahawai Player");
    expect(btn.find("svg").exists()).toBe(true);
    await btn.trigger("click");
    expect(useOverlaysStore().aboutOpen).toBe(true);
  });
});
