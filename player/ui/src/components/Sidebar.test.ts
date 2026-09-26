import { describe, expect, it } from "vitest";
import { makeTrack } from "../test/fixtures";
import { mountApp } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import Sidebar from "./Sidebar.vue";

const labels = (w: ReturnType<typeof mountApp>["wrapper"]) => w.findAll("button").map((b) => b.text().replace(/\s+\d+$/, ""));

describe("Sidebar", () => {
  it("lists the library sections and Settings", () => {
    const { wrapper } = mountApp(Sidebar);
    expect(labels(wrapper)).toEqual(["Albums", "Artists", "Playlists", "Search", "Queue", "Settings"]);
  });

  it("marks the current section and navigates on click", async () => {
    const { wrapper } = mountApp(Sidebar);
    const nav = useNavStore();
    const current = () => wrapper.findAll('[aria-current="page"]').map((b) => b.text());
    expect(current()).toEqual(["Albums"]);

    await wrapper.findAll("button").find((b) => b.text() === "Artists")!.trigger("click");
    expect(nav.view.name).toBe("artists");
    expect(current()).toEqual(["Artists"]);

    await wrapper.findAll("button").find((b) => b.text() === "Settings")!.trigger("click");
    expect(nav.view.name).toBe("settings");
    expect(current()).toEqual(["Settings"]);
  });

  it("keeps the parent section highlighted on detail pages", async () => {
    const { wrapper } = mountApp(Sidebar);
    const nav = useNavStore();
    for (const [view, section] of [["album", "Albums"], ["artist", "Artists"], ["playlist", "Playlists"]] as const) {
      nav.go(view, 7);
      await wrapper.vm.$nextTick();
      expect(wrapper.findAll('[aria-current="page"]').map((b) => b.text())).toEqual([section]);
    }
  });

  it("shows a queue count only when the queue is not empty", async () => {
    const { wrapper } = mountApp(Sidebar);
    const queue = useQueueStore();
    expect(wrapper.find('[data-testid="queue-count"]').exists()).toBe(false);
    queue.tracks = [makeTrack(), makeTrack(), makeTrack()];
    await wrapper.vm.$nextTick();
    expect(wrapper.get('[data-testid="queue-count"]').text()).toBe("3");
  });

  it("left-aligns nav labels (regression: they were centred)", () => {
    const { wrapper } = mountApp(Sidebar);
    const cls = wrapper.findAll("button")[0].classes();
    expect(cls).toContain("justify-between");
    expect(cls).not.toContain("justify-center");
  });
});
