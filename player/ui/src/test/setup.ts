import { enableAutoUnmount } from "@vue/test-utils";
import { afterEach, beforeEach, vi } from "vitest";
import { tauri } from "./tauri-mock";
import { MockEventSource, resetEventSourceMock } from "./eventsource-mock";
import { resetServerEventListenersForTest } from "../api";
import { resetResizeObserverMock, ResizeObserverStub } from "./resizeobserver-mock";
import { resetUiStateForTest } from "../lib/uiState";

vi.mock("@tauri-apps/api/core", async () => {
  const { tauri } = await import("./tauri-mock");
  return {
    invoke: (cmd: string, args?: Record<string, unknown>) => tauri.invoke(cmd, args),
    convertFileSrc: (p: string, scheme = "asset") => `${scheme}://localhost/${p}`,
  };
});

vi.mock("@tauri-apps/api/event", async () => {
  const { tauri } = await import("./tauri-mock");
  return {
    listen: (event: string, cb: (e: { payload: unknown }) => void) => tauri.listen(event, cb),
  };
});

// Browser APIs Reka UI (floating-ui, pointer capture, focus handling) expects
// and happy-dom does not implement.
vi.stubGlobal("ResizeObserver", ResizeObserverStub);
// `setBaseUrl` (api.ts) opens a catalog-events SSE connection on every call,
// including in every test's setup — happy-dom has no EventSource at all.
vi.stubGlobal("EventSource", MockEventSource);
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};
// happy-dom has no real layout engine, so every element's offsetWidth/Height
// is 0. @tanstack/vue-virtual reads these synchronously on mount (before its
// ResizeObserver — itself a no-op above — ever fires) to get an initial
// viewport size; without this, AlbumsView's virtualizer would see a 0×0
// viewport and never realize any rows, in every test that mounts it.
Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, value: 1200 });
Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, value: 900 });
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

// Node's experimental global `localStorage` shadows happy-dom's and lacks
// clear(); use a plain in-memory Storage so tests are hermetic.
class MemoryStorage implements Storage {
  private m = new Map<string, string>();
  get length(): number {
    return this.m.size;
  }
  clear(): void {
    this.m.clear();
  }
  getItem(k: string): string | null {
    return this.m.get(k) ?? null;
  }
  key(i: number): string | null {
    return [...this.m.keys()][i] ?? null;
  }
  removeItem(k: string): void {
    this.m.delete(k);
  }
  setItem(k: string, v: string): void {
    this.m.set(k, String(v));
  }
}
vi.stubGlobal("localStorage", new MemoryStorage());

// No test may reach the network: every request fails like an unreachable
// server unless the test installs its own routes with `mockFetch`.
const offline = async (): Promise<Response> => {
  throw new TypeError("network disabled in tests");
};

beforeEach(() => {
  vi.stubGlobal("fetch", offline);
  localStorage.clear();
  tauri.reset();
  resetEventSourceMock();
  resetResizeObserverMock();
  resetServerEventListenersForTest();
  resetUiStateForTest();
  // Outside Tauri unless a test opts in.
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

// Unmount every wrapper after each test so stale components (and their store
// subscriptions / portals) never react to the next test. afterEach hooks run
// in reverse order, so registering this LAST makes it run FIRST, before the
// body is cleared above.
enableAutoUnmount(afterEach);
