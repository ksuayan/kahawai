import { describe, expect, it } from "vitest";
import { makeTrack, mockFetch } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import GenreDetail from "./GenreDetail.vue";

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
  it("loads the first page, then more on request", async () => {
    const calls = routes();
    const { wrapper } = mountApp(GenreDetail, { name: "R&B" });
    await settle();
    expect(calls[0].url).toContain("/api/genres/R%26B/tracks?page=1&per_page=200");
    expect(wrapper.get("h2").text()).toBe("R&B");
    expect(wrapper.text()).toContain("250 tracks");
    expect(wrapper.findAll("[data-playable]")).toHaveLength(200);
    expect(wrapper.text()).toContain("50 not shown yet");

    const more = wrapper.findAll("button").find((b) => b.text() === "Show 50 more")!;
    await more.trigger("click");
    await settle();
    expect(wrapper.findAll("[data-playable]")).toHaveLength(250);
    expect(wrapper.text()).not.toContain("not shown yet");
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
