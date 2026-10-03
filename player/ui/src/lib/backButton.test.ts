import { describe, expect, it, vi } from "vitest";
import { mountApp, settle } from "../test/helpers";
import { useNavStore } from "../stores/nav";
import NowPlayingSheet from "../components/NowPlayingSheet.vue";
import { handleBack, installBackHandler } from "./backButton";

vi.mock("./breakpoint", () => ({ useBreakpoint: () => ({ isPhone: { value: true } }) }));

describe("Android Back", () => {
  it("closes the open sheet first, without leaving the page", async () => {
    const { wrapper } = mountApp(NowPlayingSheet, { open: true });
    const nav = useNavStore();
    nav.go("albums");
    nav.go("album", 7);
    await settle();
    expect(handleBack()).toBe(true);
    await settle();
    expect(wrapper.emitted("update:open")?.[0]).toEqual([false]);
    expect(nav.view).toEqual({ name: "album", id: 7 });
  });

  it("then goes back one step of the breadcrumb", async () => {
    mountApp(NowPlayingSheet, { open: false });
    const nav = useNavStore();
    nav.go("albums");
    nav.go("album", 7);
    await settle();
    expect(handleBack()).toBe(true);
    expect(nav.view.name).toBe("albums");
    expect(nav.trail).toHaveLength(1);
  });

  it("leaves it to Android at the start of a section", async () => {
    mountApp(NowPlayingSheet, { open: false });
    useNavStore().go("settings");
    await settle();
    expect(handleBack()).toBe(false);
  });

  it("is reachable from the Android shell", () => {
    mountApp(NowPlayingSheet, { open: false });
    installBackHandler();
    const back = (window as unknown as { kahawaiBack?: () => boolean }).kahawaiBack;
    expect(back).toBe(handleBack);
  });
});
