import { describe, expect, it, vi } from "vitest";
import { mountApp, settle, dialog } from "../test/helpers";
import NowPlayingSheet from "./NowPlayingSheet.vue";

vi.mock("../lib/breakpoint", () => ({ useBreakpoint: () => ({ isPhone: { value: true } }) }));

describe("NowPlayingSheet", () => {
  it("renders as a dialog when open", async () => {
    mountApp(NowPlayingSheet, { open: true });
    await settle();
    expect(dialog()).not.toBeNull();
    expect(document.body.querySelector('[data-testid="now-playing-sheet"]')).not.toBeNull();
    expect(document.body.querySelector('[data-testid="sheet-handle"]')).not.toBeNull();
  });

  it("renders nothing when closed", async () => {
    mountApp(NowPlayingSheet, { open: false });
    await settle();
    expect(document.body.querySelector('[data-testid="now-playing-sheet"]')).toBeNull();
  });

  it("emits update:open false when the scrim is clicked", async () => {
    const { wrapper } = mountApp(NowPlayingSheet, { open: true });
    await settle();
    const scrim = document.body.querySelector('[data-testid="sheet-scrim"]') as HTMLElement;
    expect(scrim).not.toBeNull();
    // Reka closes on pointerdown outside the content.
    scrim.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    await settle();
    expect(wrapper.emitted("update:open")).toBeTruthy();
  });

  it("springs back and stays open when the browser cancels the drag", async () => {
    const { wrapper } = mountApp(NowPlayingSheet, { open: true });
    await settle();
    const handle = document.body.querySelector('[data-testid="sheet-handle"]') as HTMLElement;
    const sheet = document.body.querySelector('[data-testid="now-playing-sheet"]') as HTMLElement;
    handle.setPointerCapture = () => {}; // not in jsdom
    expect(handle.classList).toContain("touch-none"); // the WebView must not take it as a scroll
    handle.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, clientY: 100 }));
    handle.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, clientY: 400 }));
    await settle();
    expect(sheet.style.transform).toBe("translateY(300px)");
    handle.dispatchEvent(new PointerEvent("pointercancel", { bubbles: true }));
    await settle();
    expect(sheet.style.transform).toBe("");
    expect(wrapper.emitted("update:open")).toBeFalsy();
  });
});
