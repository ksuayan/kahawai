import { describe, expect, it } from "vitest";
import { mountApp, openSelect, options, pick, settle } from "../test/helpers";
import { SORT_OPTIONS } from "../lib/sorting";
import ListToolbar from "./ListToolbar.vue";

describe("ListToolbar", () => {
  it("switches list and grid, with tooltips, and marks the current one", async () => {
    const { wrapper } = mountApp(ListToolbar, { layout: "grid", sort: "artist-asc", sortOptions: SORT_OPTIONS });
    const list = wrapper.get('button[aria-label="List"]');
    const grid = wrapper.get('button[aria-label="Grid"]');
    expect([list.attributes("title"), grid.attributes("title")]).toEqual(["List", "Grid"]);
    expect(grid.attributes("aria-pressed")).toBe("true");
    expect(list.attributes("aria-pressed")).not.toBe("true");
    await list.trigger("click");
    expect(wrapper.emitted("update:layout")).toEqual([["list"]]);
  });

  it("offers the six sort orders and reports the choice", async () => {
    const { wrapper } = mountApp(ListToolbar, { layout: "grid", sort: "artist-asc", sortOptions: SORT_OPTIONS });
    await openSelect(wrapper.get('[role="combobox"]').element as HTMLElement);
    expect(options().map((o) => o.textContent?.trim())).toEqual(SORT_OPTIONS.map((o) => o.label));
    pick(options().find((o) => o.textContent?.includes("newest first"))!);
    await settle();
    expect(wrapper.emitted("update:sort")).toEqual([["year-desc"]]);
  });

  it("hides the sort menu when there's nothing to sort by", () => {
    const { wrapper } = mountApp(ListToolbar, { layout: "grid" });
    expect(wrapper.find('[aria-label="Layout"]').exists()).toBe(true);
    expect(wrapper.find('[role="combobox"]').exists()).toBe(false);
  });

  it("hides the layout toggle when there's no layout to choose", () => {
    const { wrapper } = mountApp(ListToolbar, { sort: "artist-asc", sortOptions: SORT_OPTIONS });
    expect(wrapper.find('[aria-label="Layout"]').exists()).toBe(false);
  });
});
