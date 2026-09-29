/** Leaving Albums unmounts it; coming back must restore the grid's scroll. */
import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { useLibraryStore } from "../stores/library";
import { useScrollMemoryStore } from "../stores/scrollMemory";
import { makeAlbum } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import AlbumsView from "./AlbumsView.vue";

const albums = Array.from({ length: 400 }, (_, i) => makeAlbum({ id: i, title: `Album ${i}`, artist: `Artist ${i}` }));
const seed = () => {
  useLibraryStore().albums = albums;
};
const scroller = (w: { find: (s: string) => { element: Element } }) => w.find(".overflow-y-auto").element as HTMLElement;

describe("AlbumsView scroll position", () => {
  it("is restored after leaving the view and coming back", async () => {
    const { wrapper, pinia } = mountApp(AlbumsView, {}, {}, seed);
    await settle();
    scroller(wrapper).scrollTop = 4200;
    wrapper.unmount(); // navigating to another view

    const again = mount(AlbumsView, { attachTo: document.body, global: { plugins: [pinia] } });
    await settle();
    expect(scroller(again).scrollTop).toBe(4200);
    again.unmount();
  });

  it("starts at the top the first time", async () => {
    const { wrapper } = mountApp(AlbumsView, {}, {}, seed);
    await settle();
    expect(scroller(wrapper).scrollTop).toBe(0);
    wrapper.unmount();
  });

  it("does not forget the position when the view was mounted before the library loaded", async () => {
    const { wrapper, pinia } = mountApp(AlbumsView, {}, {}, () => {
      seed();
      useScrollMemoryStore().set("albums", 3000);
      useLibraryStore().loading = true;
    });
    await settle();
    wrapper.unmount(); // left again before the grid ever appeared
    expect(useScrollMemoryStore(pinia).get("albums")).toBe(3000);
  });

  it("restores once the grid appears if the library was still loading on return", async () => {
    const { wrapper } = mountApp(AlbumsView, {}, {}, () => {
      useScrollMemoryStore().set("albums", 3000);
      useLibraryStore().loading = true;
    });
    await settle();
    useLibraryStore().loading = false;
    useLibraryStore().albums = albums;
    await settle();
    expect(scroller(wrapper).scrollTop).toBe(3000);
    wrapper.unmount();
  });
});
