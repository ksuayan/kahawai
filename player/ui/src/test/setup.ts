import { enableAutoUnmount } from "@vue/test-utils";
import { afterEach, beforeEach, vi } from "vitest";
import { tauri } from "./tauri-mock";

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
