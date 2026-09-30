import { describe, expect, it } from "vitest";
import { mountApp, settle } from "../test/helpers";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import GenresView from "./GenresView.vue";

describe("GenresView", () => {
  it("lists genres with their track counts and opens one", async () => {
    const { wrapper } = mountApp(GenresView);
    const lib = useLibraryStore();
    lib.genres = [
      { name: "Jazz", track_count: 15064 },
      { name: "R&B", track_count: 557 },
    ];
    await settle();
    expect(wrapper.text()).toContain("2 genres");
    const chips = wrapper.findAll("li button");
    expect(chips.map((c) => c.text())).toEqual(["Jazz 15,064", "R&B 557"]);
    await chips[1].trigger("click");
    expect(useNavStore().view).toEqual({ name: "genre", id: undefined, genre: "R&B" });
  });

  it("explains an empty list", async () => {
    const { wrapper } = mountApp(GenresView);
    await settle();
    expect(wrapper.text()).toContain("No genres yet");
  });
});
