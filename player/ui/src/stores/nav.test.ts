import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { crumbLabel, MAX_TRAIL, useNavStore, type Crumb } from "./nav";

const KEY = "kahawai.nav";
/** A fresh store, as on launch (it reads what was saved). */
function launch() {
  setActivePinia(createPinia());
  return useNavStore();
}
const names = (trail: Crumb[]) => trail.map((c) => (c.id != null ? `${c.name}:${c.id}` : c.genre ? `${c.name}:${c.genre}` : c.name));

beforeEach(() => localStorage.clear());

describe("nav: the breadcrumb trail", () => {
  it("starts on Albums with a one-step trail", () => {
    const nav = launch();
    expect(nav.view).toEqual({ name: "albums" });
    expect(names(nav.trail)).toEqual(["albums"]);
    expect(nav.section).toBe("albums");
  });

  it("a section starts a new trail; a page opened from it is a step further", () => {
    const nav = launch();
    nav.go("artists");
    nav.go("artist", 3);
    nav.go("album", 9);
    expect(names(nav.trail)).toEqual(["artists", "artist:3", "album:9"]);
    expect(nav.view).toEqual({ name: "album", id: 9 });
    expect(nav.section).toBe("artists");
    nav.go("genres");
    expect(names(nav.trail)).toEqual(["genres"]);
  });

  it("going to a page already in the trail goes back to it", () => {
    const nav = launch();
    nav.go("artists");
    nav.go("artist", 3);
    nav.go("album", 9);
    nav.go("artist", 3);
    expect(names(nav.trail)).toEqual(["artists", "artist:3"]);
  });

  it("a page of the same kind as the last step takes its place", () => {
    const nav = launch();
    nav.go("albums");
    nav.go("album", 1);
    nav.go("album", 2);
    expect(names(nav.trail)).toEqual(["albums", "album:2"]);
    nav.go("genres");
    nav.go("genre", undefined, "Jazz");
    nav.go("genre", undefined, "Soul");
    expect(names(nav.trail)).toEqual(["genres", "genre:Soul"]);
  });

  it("Now Playing is a step from wherever you are", () => {
    const nav = launch();
    nav.go("search");
    nav.go("album", 4);
    nav.go("nowplaying");
    expect(names(nav.trail)).toEqual(["search", "album:4", "nowplaying"]);
    expect(nav.section).toBe("search");
  });

  it("a long wander keeps the section and the latest steps", () => {
    const nav = launch();
    nav.go("artists");
    for (let i = 1; i <= 5; i++) {
      nav.go("artist", i);
      nav.go("album", i);
    }
    expect(nav.trail).toHaveLength(MAX_TRAIL);
    expect(names(nav.trail)).toEqual(["artists", "album:3", "artist:4", "album:4", "artist:5", "album:5"]);
  });

  it("goes back to a step, dropping the ones after it", () => {
    const nav = launch();
    nav.go("artists");
    nav.go("artist", 3);
    nav.go("album", 9);
    nav.goToCrumb(1);
    expect(nav.view).toEqual({ name: "artist", id: 3 });
    expect(names(nav.trail)).toEqual(["artists", "artist:3"]);
    nav.goToCrumb(7); // no such step: nothing happens
    expect(names(nav.trail)).toEqual(["artists", "artist:3"]);
  });

  it("a page names its step, and the name is kept when it is an ancestor", () => {
    const nav = launch();
    nav.go("artists");
    nav.go("artist", 3);
    nav.setLabel({ name: "artist", id: 3 }, "Miles Davis");
    nav.go("album", 9);
    nav.setLabel({ name: "album", id: 9 }, "Kind of Blue");
    expect(nav.trail.map(crumbLabel)).toEqual(["Artists", "Miles Davis", "Kind of Blue"]);
    // A page that is not in the trail (a late answer for a page left behind) changes nothing.
    nav.setLabel({ name: "album", id: 1 }, "Elsewhere");
    expect(nav.trail.map(crumbLabel)).toEqual(["Artists", "Miles Davis", "Kind of Blue"]);
  });

  it("before a page names itself, its step says what kind of page it is", () => {
    expect(crumbLabel({ name: "album", id: 1 })).toBe("Album");
    expect(crumbLabel({ name: "genre", genre: "Jazz" })).toBe("Jazz");
    expect(crumbLabel({ name: "nowplaying" })).toBe("Now Playing");
    expect(crumbLabel({ name: "queue", label: "ignored" })).toBe("Queue");
  });
});

describe("nav: saved across launches", () => {
  it("reopens on the same page with the same trail and names", () => {
    const nav = launch();
    nav.go("artists");
    nav.go("artist", 3);
    nav.setLabel({ name: "artist", id: 3 }, "Miles Davis");
    nav.go("album", 9);
    const again = launch();
    expect(again.view).toEqual({ name: "album", id: 9 });
    expect(names(again.trail)).toEqual(["artists", "artist:3", "album:9"]);
    expect(again.trail[1]!.label).toBe("Miles Davis");
  });

  it("a page saved by an earlier version (no trail) gets its section as the trail", () => {
    localStorage.setItem(KEY, JSON.stringify({ name: "album", id: 5 }));
    const nav = launch();
    expect(nav.view).toEqual({ name: "album", id: 5 });
    expect(names(nav.trail)).toEqual(["albums", "album:5"]);
  });

  it("a trail that does not fit the page, or is damaged, is rebuilt", () => {
    for (const trail of [
      [{ name: "artists" }, { name: "album", id: 6 }], // ends somewhere else
      [{ name: "album", id: 5 }], // does not start at a section
      [{ name: "albums" }, { name: "album" }, { name: "album", id: 5 }], // an album without an id
      "nonsense",
      Array.from({ length: MAX_TRAIL + 1 }, () => ({ name: "albums" })),
    ]) {
      localStorage.setItem(KEY, JSON.stringify({ name: "album", id: 5, trail }));
      expect(names(launch().trail)).toEqual(["albums", "album:5"]);
    }
  });

  it("an unreadable or unknown saved page starts on Albums", () => {
    for (const saved of ["{not json", JSON.stringify({ name: "nowhere" }), JSON.stringify({ name: "album" })]) {
      localStorage.setItem(KEY, saved);
      const nav = launch();
      expect(nav.view).toEqual({ name: "albums" });
      expect(names(nav.trail)).toEqual(["albums"]);
    }
  });
});
