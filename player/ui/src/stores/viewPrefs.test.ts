import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { nextTick } from "vue";
import { useViewPrefsStore } from "./viewPrefs";

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
});

describe("viewPrefs", () => {
  it("defaults, then remembers choices across launches", async () => {
    const v = useViewPrefsStore();
    expect(v.prefs.albumsLayout).toBe("grid");
    expect(v.prefs.albumsSort).toBe("artist-asc");
    v.prefs.albumsLayout = "list";
    v.prefs.albumsSort = "year-desc";
    await nextTick();
    setActivePinia(createPinia());
    const again = useViewPrefsStore();
    expect([again.prefs.albumsLayout, again.prefs.albumsSort]).toEqual(["list", "year-desc"]);
  });

  it("remembers track views too, including their natural order", async () => {
    const v = useViewPrefsStore();
    expect([v.prefs.queueSort, v.prefs.queueLayout]).toEqual(["default", "list"]);
    v.prefs.queueSort = "year-desc";
    v.prefs.searchLayout = "grid";
    await nextTick();
    setActivePinia(createPinia());
    const again = useViewPrefsStore();
    expect([again.prefs.queueSort, again.prefs.searchLayout]).toEqual(["year-desc", "grid"]);
  });

  it("ignores stored values it doesn't know", () => {
    localStorage.setItem("kahawai.viewPrefs", JSON.stringify({ albumsLayout: "carousel", albumsSort: "mood-asc" }));
    const v = useViewPrefsStore();
    expect([v.prefs.albumsLayout, v.prefs.albumsSort]).toEqual(["grid", "artist-asc"]);
  });
});
