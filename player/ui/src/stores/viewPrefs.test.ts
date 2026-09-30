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

  it("ignores stored values it doesn't know", () => {
    localStorage.setItem("kahawai.viewPrefs", JSON.stringify({ albumsLayout: "carousel", albumsSort: "mood-asc" }));
    const v = useViewPrefsStore();
    expect([v.prefs.albumsLayout, v.prefs.albumsSort]).toEqual(["grid", "artist-asc"]);
  });
});
