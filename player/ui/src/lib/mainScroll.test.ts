/** Artists and Playlists scroll in App's shared <main>; they must come back where they were. */
import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { ref } from "vue";
import { MAIN_SCROLL } from "./mainScroll";
import { useLibraryStore } from "../stores/library";
import { usePlaylistsStore } from "../stores/playlists";
import { useScrollMemoryStore } from "../stores/scrollMemory";
import ArtistsView from "../components/ArtistsView.vue";
import PlaylistsView from "../components/PlaylistsView.vue";
import { makeAlbum } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import type { Component } from "vue";
import type { Pinia } from "pinia";

/** A stand-in for App's <main>: one element that outlives each view. */
function makeMain() {
  const el = document.createElement("main");
  document.body.appendChild(el);
  return { el, provide: { [MAIN_SCROLL as symbol]: ref(el) } };
}

function mountIn(component: Component, pinia: Pinia, main: ReturnType<typeof makeMain>) {
  return mount(component, { attachTo: document.body, global: { plugins: [pinia], provide: main.provide } });
}

function seedLibrary() {
  const lib = useLibraryStore();
  lib.artists = Array.from({ length: 60 }, (_, i) => ({ id: i, name: `Artist ${i}`, album_count: 1, track_count: 1 })) as never;
  lib.albums = [makeAlbum()];
}

describe("ArtistsView scroll position (its own virtualized scroller)", () => {
  const scroller = (w: { find: (s: string) => { element: Element } }) => w.find(".overflow-y-auto").element as HTMLElement;

  it("comes back to where it was left", async () => {
    const { wrapper, pinia } = mountApp(ArtistsView, {}, {}, seedLibrary);
    await settle();
    scroller(wrapper).scrollTop = 1800;
    wrapper.unmount();
    const again = mount(ArtistsView, { attachTo: document.body, global: { plugins: [pinia] } });
    await settle();
    expect(scroller(again).scrollTop).toBe(1800);
    again.unmount();
  });

  it("starts at the top the first time", async () => {
    const { wrapper } = mountApp(ArtistsView, {}, {}, seedLibrary);
    await settle();
    expect(scroller(wrapper).scrollTop).toBe(0);
    wrapper.unmount();
  });

  it("keeps the offset if left while the artists were still loading", async () => {
    const { wrapper, pinia } = mountApp(ArtistsView, {}, {}, () => {
      seedLibrary();
      useScrollMemoryStore().set("artists", 1200);
      useLibraryStore().loading = true;
    });
    await settle();
    wrapper.unmount();
    expect(useScrollMemoryStore(pinia).get("artists")).toBe(1200);
  });
});

describe("PlaylistsView scroll position", () => {
  const ready = () => {
    const p = usePlaylistsStore();
    p.loaded = true;
    p.items = Array.from({ length: 40 }, (_, i) => ({ id: i, name: `List ${i}`, track_ids: [1, 2, 3] })) as never;
  };

  it("comes back to where it was left", async () => {
    const main = makeMain();
    const { pinia } = mountApp(PlaylistsView, {}, {}, ready);
    await settle();
    const first = mountIn(PlaylistsView, pinia, main);
    await settle();
    main.el.scrollTop = 900;
    first.unmount();

    const again = mountIn(PlaylistsView, pinia, main);
    main.el.scrollTop = 0;
    await settle();
    expect(main.el.scrollTop).toBe(900);
    again.unmount();
  });

  it("restores once the playlists have loaded when it was opened before they were", async () => {
    const main = makeMain();
    const { pinia } = mountApp(PlaylistsView, {}, {}, () => {
      useScrollMemoryStore().set("playlists", 640);
    });
    const v = mountIn(PlaylistsView, pinia, main); // not loaded yet: the view starts the load itself
    await settle();
    expect(usePlaylistsStore(pinia).loaded).toBe(true);
    expect(main.el.scrollTop).toBe(640);
    v.unmount();
  });
});
