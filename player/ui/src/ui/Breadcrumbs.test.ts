import { describe, expect, it } from "vitest";
import { mountApp, settle } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import Breadcrumbs from "./Breadcrumbs.vue";

const crumbs = (w: ReturnType<typeof mountApp>["wrapper"]) => w.findAll("li").map((li) => li.text());

describe("Breadcrumbs", () => {
  it("shows the trail; each step but the last goes back to it", async () => {
    const { wrapper } = mountApp(Breadcrumbs, { section: "albums", current: "Kind of Blue" }, {}, () => {
      const nav = useNavStore();
      nav.go("artists");
      nav.go("artist", 3);
      nav.setLabel({ name: "artist", id: 3 }, "Miles Davis");
      nav.go("album", 9);
    });
    await settle();
    expect(wrapper.get("nav").attributes("aria-label")).toBe("Breadcrumb");
    expect(crumbs(wrapper)).toEqual(["Artists", "Miles Davis", "Kind of Blue"]);
    expect(wrapper.get('[aria-current="page"]').text()).toBe("Kind of Blue");
    expect(wrapper.findAll("button").map((b) => b.text())).toEqual(["Artists", "Miles Davis"]);
    await wrapper.findAll("button")[1]!.trigger("click");
    expect(useNavStore().view).toEqual({ name: "artist", id: 3 });
  });

  it("names the current step once the page knows its title", async () => {
    const { wrapper } = mountApp(Breadcrumbs, { section: "albums" }, {}, () => {
      useNavStore().go("albums");
      useNavStore().go("album", 9);
    });
    expect(crumbs(wrapper)).toEqual(["Albums", "Album"]);
    await wrapper.setProps({ current: "Kind of Blue" });
    expect(crumbs(wrapper)).toEqual(["Albums", "Kind of Blue"]);
    expect(useNavStore().trail[1]!.label).toBe("Kind of Blue");
  });

  it("with no trail, shows the page's section and the page", async () => {
    const { wrapper } = mountApp(Breadcrumbs, { section: "playlists", current: "Road trip" });
    expect(crumbs(wrapper)).toEqual(["Playlists", "Road trip"]);
    await wrapper.get("button").trigger("click");
    expect(useNavStore().view.name).toBe("playlists");
  });
});
