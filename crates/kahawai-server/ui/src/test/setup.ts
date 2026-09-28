import { enableAutoUnmount } from "@vue/test-utils";
import { afterEach, beforeEach, vi } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { dialog } from "./dialog-mock";

vi.mock("@tauri-apps/api/core", async () => {
  const { tauri } = await import("@pw/test/tauri-mock");
  return {
    invoke: (cmd: string, args?: Record<string, unknown>) => tauri.invoke(cmd, args),
  };
});

vi.mock("@tauri-apps/plugin-dialog", async () => {
  const { dialog } = await import("./dialog-mock");
  return { open: () => dialog.open() };
});

// Browser APIs Reka UI (floating-ui, pointer capture, focus handling) expects
// and happy-dom does not implement.
class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
vi.stubGlobal("ResizeObserver", ResizeObserverStub);
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};
window.matchMedia ??= ((q: string) => ({
  matches: false,
  media: q,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
  onchange: null,
  dispatchEvent: () => false,
})) as typeof window.matchMedia;

beforeEach(() => {
  tauri.reset();
  dialog.reset();
  // Outside Tauri unless a test opts in.
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

// Unmount every wrapper after each test so stale components never react to
// the next test. afterEach hooks run in reverse order, so registering this
// LAST makes it run FIRST, before the body is cleared above.
enableAutoUnmount(afterEach);
