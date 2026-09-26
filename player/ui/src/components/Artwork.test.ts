import { describe, expect, it } from "vitest";
import { mountApp, settle } from "../test/helpers";
import { setBaseUrl } from "../api";
import AlbumCard from "./AlbumCard.vue";
import Artwork from "./Artwork.vue";
import { makeAlbum } from "../test/fixtures";

describe("Artwork", () => {
  it("loads the cover from the server outside the app", () => {
    setBaseUrl("http://server:8080");
    const { wrapper } = mountApp(Artwork, { hash: "abc123" });
    expect(wrapper.get("img").attributes("src")).toBe("http://server:8080/api/artwork/abc123");
  });

  it("loads the cover through the disk-cache protocol inside the app", () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const { wrapper } = mountApp(Artwork, { hash: "abc123" });
    expect(wrapper.get("img").attributes("src")).toBe("artwork://localhost/abc123");
  });

  it("shows a placeholder when there is no hash", () => {
    const { wrapper } = mountApp(Artwork, { hash: null });
    expect(wrapper.find("img").exists()).toBe(false);
    expect(wrapper.find("svg").exists()).toBe(true);
  });

  it("falls back to the placeholder when the image fails to load", async () => {
    const { wrapper } = mountApp(Artwork, { hash: "gone" });
    await wrapper.get("img").trigger("error");
    expect(wrapper.find("img").exists()).toBe(false);
    expect(wrapper.find("svg").exists()).toBe(true);
  });

  it("gives a different cover a fresh chance after one failed", async () => {
    const { wrapper } = mountApp(Artwork, { hash: "gone" });
    await wrapper.get("img").trigger("error");
    await wrapper.setProps({ hash: "good" });
    await settle();
    expect(wrapper.get("img").attributes("src")).toContain("/good");
  });

  it("sizes itself in pixels, or fills the width as a square when fluid", () => {
    const fixed = mountApp(Artwork, { hash: "a", size: 44, radius: 6 }).wrapper;
    expect(fixed.attributes("style")).toContain("width: 44px");
    expect(fixed.attributes("style")).toContain("height: 44px");
    expect(fixed.attributes("style")).toContain("border-radius: 6px");
    const fluid = mountApp(Artwork, { hash: "a", fluid: true, radius: 8 }).wrapper;
    expect(fluid.classes()).toEqual(expect.arrayContaining(["aspect-square", "w-full"]));
    expect(fluid.attributes("style")).not.toContain("width: ");
  });

  it("uses the alt text and does not let the image be dragged", () => {
    const { wrapper } = mountApp(Artwork, { hash: "a", alt: "Stranger in the Alps" });
    expect(wrapper.get("img").attributes("alt")).toBe("Stranger in the Alps");
    expect(wrapper.get("img").attributes("draggable")).toBe("false");
  });
});

describe("AlbumCard", () => {
  it("shows the title and subtitle and reports which album was opened", async () => {
    const album = makeAlbum({ id: 12, title: "Stranger in the Alps" });
    const { wrapper } = mountApp(AlbumCard, { album, subtitle: "Phoebe Bridgers · 2017 · 3 tracks" });
    expect(wrapper.text()).toContain("Stranger in the Alps");
    expect(wrapper.text()).toContain("Phoebe Bridgers · 2017 · 3 tracks");
    await wrapper.trigger("click");
    expect(wrapper.emitted("open")).toEqual([[12]]);
  });
});
