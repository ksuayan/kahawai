import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import { useSetupStore } from "../stores/setup";
import AboutDialog from "./AboutDialog.vue";
import StatusView from "./StatusView.vue";

const content = () => document.body.querySelector('[data-testid="about-content"]') as HTMLElement;
const image = () => document.body.querySelector('[data-testid="about-image"]') as HTMLImageElement | null;

describe("Server app About", () => {
  it("shows the splash artwork above the title, the version, and the notices tab", async () => {
    mountApp(AboutDialog);
    const setup = useSetupStore();
    setup.aboutOpen = true;
    await settle();
    expect(image()!.getAttribute("src")).toBe("/splash.webp");
    expect(image()!.compareDocumentPosition(content()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(content().querySelector("h1")!.textContent).toBe("Kahawai Server");
    expect(content().textContent).toMatch(/Version \d+\.\d+\.\d+/);
    expect(content().textContent).not.toContain("{{version}}");

    (document.body.querySelector('[data-testid="about-tab-notices"]') as HTMLElement).click();
    await settle();
    expect(content().querySelector("h1")!.textContent).toBe("Open-source notices");
    expect(content().textContent).toContain("Kahawai Server includes the following third-party software");
    expect(image()).toBeNull();
  });

  it("states the license and the network source offer, and shows the full text", async () => {
    mountApp(AboutDialog);
    useSetupStore().aboutOpen = true;
    await settle();
    const text = content().textContent!;
    expect(text).toContain("GNU Affero General Public License");
    expect(text).toContain("without any warranty");
    expect(text).toContain("https://github.com/ksuayan/kahawai");
    expect(text).toContain("used over a network");
    expect(text).not.toContain("All rights reserved");
    (document.body.querySelector('[data-testid="about-tab-license"]') as HTMLElement).click();
    await settle();
    expect(document.body.querySelector('[data-testid="about-license"]')!.textContent).toContain("GNU AFFERO GENERAL PUBLIC LICENSE");
  });

  it("ends the About page with the disclaimers", async () => {
    mountApp(AboutDialog);
    useSetupStore().aboutOpen = true;
    await settle();
    const headings = [...content().querySelectorAll("h2")].map((h) => h.textContent);
    expect(headings.at(-1)).toBe("Disclaimers");
    expect(content().textContent).toContain("No warranty.");
    expect(content().textContent).toContain("Your hearing and your equipment.");
  });

  it("shows the running server's build when it's known", async () => {
    mountApp(AboutDialog);
    const setup = useSetupStore();
    setup.identity = {
      service: "kahawai-server",
      name: "Kahawai Server",
      version: "0.1.0",
      api_version: 1,
      build: { commit: "cd9b827", dirty: false, built_at: "2026-09-30T19:02:11Z", profile: "release", target: "aarch64-apple-darwin" },
      catalog_id: "3f1c",
      started_at: 0,
    };
    setup.aboutOpen = true;
    await settle();
    expect(document.body.querySelector('[data-testid="about-build"]')!.textContent).toContain("Build cd9b827");
  });

  it("opens from the About button next to the tabs", async () => {
    tauri.on("setup_recent_scans", []);
    const { wrapper } = mountApp(StatusView);
    await wrapper.get('[data-testid="about-button"]').trigger("click");
    expect(useSetupStore().aboutOpen).toBe(true);
  });
});
