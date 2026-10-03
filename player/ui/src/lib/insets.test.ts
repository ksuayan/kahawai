import { afterEach, describe, expect, it } from "vitest";
import { applyNativeInsets, watchNativeInsets } from "./insets";

const w = window as unknown as { KahawaiInsets?: { get(): string } };
afterEach(() => {
  delete w.KahawaiInsets;
  document.documentElement.removeAttribute("style");
});

describe("insets from the Android app", () => {
  it("does nothing without the bridge (desktop, browser)", () => {
    expect(applyNativeInsets()).toBeNull();
    expect(document.documentElement.style.getPropertyValue("--sat")).toBe("");
  });

  it("sets the safe-area variables from the app's insets, never below the WebView's own", () => {
    w.KahawaiInsets = { get: () => JSON.stringify({ top: 24.4, right: 0, bottom: 20, left: -3 }) };
    expect(applyNativeInsets()).toEqual({ top: 24, right: 0, bottom: 20, left: 0 });
    const s = document.documentElement.style;
    expect(s.getPropertyValue("--sat")).toBe("max(env(safe-area-inset-top, 0px), 24px)");
    expect(s.getPropertyValue("--sab")).toBe("max(env(safe-area-inset-bottom, 0px), 20px)");
    expect(s.getPropertyValue("--sal")).toBe("max(env(safe-area-inset-left, 0px), 0px)");
  });

  it("follows the app when the bars change", () => {
    let top = 24;
    w.KahawaiInsets = { get: () => JSON.stringify({ top, right: 0, bottom: 0, left: 0 }) };
    watchNativeInsets();
    top = 48;
    window.dispatchEvent(new Event("kahawai-insets"));
    expect(document.documentElement.style.getPropertyValue("--sat")).toBe("max(env(safe-area-inset-top, 0px), 48px)");
  });

  it("ignores an unreadable answer", () => {
    w.KahawaiInsets = { get: () => "not json" };
    expect(applyNativeInsets()).toBeNull();
  });
});
