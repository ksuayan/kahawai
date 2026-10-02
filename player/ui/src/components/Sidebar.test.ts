import { describe, expect, it } from "vitest";
import { makeTrack } from "../test/fixtures";
import { mountApp } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import Sidebar from "./Sidebar.vue";

const labels = (w: ReturnType<typeof mountApp>["wrapper"]) => w.findAll("button").filter((b) => !["theme-toggle", "about-button"].includes(b.attributes("data-testid") ?? "")).map((b) => b.text().replace(/\s+\d+$/, ""));

describe("Sidebar", () => {
  it("lists the library sections and Settings", () => {
    const { wrapper } = mountApp(Sidebar);
    expect(labels(wrapper)).toEqual(["Albums", "Artists", "Genres", "Playlists", "Audiobooks", "Radio", "Search", "Queue", "Settings"]);
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
    for (const [from, view, section] of [["albums", "album", "Albums"], ["artists", "artist", "Artists"], ["genres", "genre", "Genres"], ["playlists", "playlist", "Playlists"], ["audiobooks", "audiobook", "Audiobooks"]] as const) {
      nav.go(from);
      nav.go(view, 7);
      await wrapper.vm.$nextTick();
      expect(wrapper.findAll('[aria-current="page"]').map((b) => b.text())).toEqual([section]);
    }
  });

  it("keeps the section the breadcrumb started from: an album opened from an artist keeps Artists lit", async () => {
    const { wrapper } = mountApp(Sidebar);
    const nav = useNavStore();
    nav.go("artists");
    nav.go("artist", 3);
    nav.go("album", 9);
    await wrapper.vm.$nextTick();
    expect(wrapper.findAll('[aria-current="page"]').map((b) => b.text())).toEqual(["Artists"]);
  });

  it("shows a queue count only when the queue is not empty", async () => {
    const { wrapper } = mountApp(Sidebar);
    const queue = useQueueStore();
    expect(wrapper.find('[data-testid="queue-count"]').exists()).toBe(false);
    queue.tracks = [makeTrack(), makeTrack(), makeTrack()];
    await wrapper.vm.$nextTick();
    expect(wrapper.get('[data-testid="queue-count"]').text()).toBe("3");
  });

  it("gives every entry an icon, and Settings a gear", () => {
    const { wrapper } = mountApp(Sidebar);
    const icons = wrapper.findAll("button").filter((b) => b.text() !== "").map((b) => ({
      label: b.text().replace(/\s+\d+$/, ""),
      icon: b.find("svg").classes().find((c) => /^lucide-[a-z0-9-]+$/.test(c) && !c.endsWith("-icon")),
    }));
    expect(icons).toEqual([
      { label: "Albums", icon: "lucide-disc-3" },
      { label: "Artists", icon: "lucide-mic-vocal" },
      { label: "Genres", icon: "lucide-tags" },
      { label: "Playlists", icon: "lucide-list-music" },
      { label: "Audiobooks", icon: "lucide-book-open" },
      { label: "Radio", icon: "lucide-radio" },
      { label: "Search", icon: "lucide-search" },
      { label: "Queue", icon: "lucide-list-ordered" },
      { label: "Settings", icon: "lucide-settings" },
    ]);
    // Icons are decoration; the visible label names the button.
    for (const b of wrapper.findAll("button")) expect(b.find("svg").attributes("aria-hidden")).toBe("true");
  });

  it("puts the icon to the left of the label, and the queue count on the right", async () => {
    const { wrapper } = mountApp(Sidebar);
    useQueueStore().tracks = [makeTrack()];
    await wrapper.vm.$nextTick();
    const queue = wrapper.findAll("button").find((b) => b.text().startsWith("Queue"))!;
    const children = Array.from(queue.element.children);
    expect(children[0].querySelector("svg")).not.toBeNull();
    expect(children[1].getAttribute("data-testid")).toBe("queue-count");
  });

  it("left-aligns nav labels (regression: they were centred)", () => {
    const { wrapper } = mountApp(Sidebar);
    const cls = wrapper.findAll("button")[0].classes();
    expect(cls).toContain("justify-between");
    expect(cls).not.toContain("justify-center");
  });
});
