import { describe, expect, it } from "vitest";
import { makeTrack, mockFetch } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import { useViewPrefsStore } from "../stores/viewPrefs";
import GenreDetail from "./GenreDetail.vue";
import VirtualList from "./VirtualList.vue";

const tracks = Array.from({ length: 250 }, (_, i) => makeTrack({ title: `T${i}` }));
const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

function routes() {
  return mockFetch({
    "/api/genres/R%26B/tracks": (url: string) => {
      const q = new URL(url).searchParams;
      const page = Number(q.get("page"));
      const per = Number(q.get("per_page"));
      const items = tracks.slice((page - 1) * per, page * per);
      return json({ items, page, per_page: per, total: tracks.length });
    },
    "/api/playlists": [],
  });
}

describe("GenreDetail", () => {
  it("loads the first page, then the next as the list nears its end", async () => {
    const calls = routes();
    const { wrapper } = mountApp(GenreDetail, { name: "R&B" });
    await settle();
    expect(calls[0].url).toContain("/api/genres/R%26B/tracks?page=1&per_page=200&sort=artist&order=asc");
    expect(wrapper.get("h2").text()).toBe("R&B");
    expect(wrapper.text()).toContain("250 tracks");
    const items = () => (wrapper.findComponent(VirtualList).props() as { items: unknown[] }).items;
    const nearEnd = () => (wrapper.findComponent(VirtualList).vm as unknown as { $emit: (e: string) => void }).$emit("near-end");
    expect(items()).toHaveLength(200);

    nearEnd();
    nearEnd(); // one page at a time
    await settle();
    const pages = calls.filter((c) => c.url.includes("/tracks?")).map((c) => new URL(c.url).searchParams.get("page"));
    expect(pages).toEqual(["1", "2"]);
    expect(items()).toHaveLength(250);

    nearEnd();
    await settle();
    expect(calls.filter((c) => c.url.includes("/tracks?"))).toHaveLength(2); // nothing left to load
  });

  it("re-sorts on the server when the sort changes", async () => {
    const calls = routes();
    mountApp(GenreDetail, { name: "R&B" });
    await settle();
    useViewPrefsStore().prefs.genreTracksSort = "year-desc";
    await settle();
    expect(calls.at(-1)!.url).toContain("page=1&per_page=200&sort=year&order=desc");
  });

  it("shows the server's error, and goes back to the list", async () => {
    mockFetch({ "/api/genres/Polka/tracks": () => json({ error: "genre Polka" }, 404), "/api/playlists": [] });
    const { wrapper } = mountApp(GenreDetail, { name: "Polka" });
    await settle();
    expect(wrapper.get('[role="alert"]').text()).toBe("genre Polka");
    await wrapper.findAll("button").find((b) => /Genres/.test(b.text()))!.trigger("click");
    expect(useNavStore().view.name).toBe("genres");
  });
});
