import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useToastsStore } from "./toasts";

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
});
afterEach(() => vi.useRealTimers());

describe("toasts", () => {
  it("auto-dismisses info toasts after 6s and errors after 12s", () => {
    const t = useToastsStore();
    t.push("info", "hello");
    t.push("error", "boom");
    vi.advanceTimersByTime(6001);
    expect(t.toasts.map((x) => x.title)).toEqual(["boom"]);
    vi.advanceTimersByTime(6000);
    expect(t.toasts).toHaveLength(0);
  });

  it("keeps progress toasts until dismissed and updates them in place", () => {
    const t = useToastsStore();
    const id = t.push("progress", "Scanning", { progress: 0.1 });
    vi.advanceTimersByTime(60_000);
    expect(t.toasts).toHaveLength(1);
    t.update(id, { progress: 0.8, detail: "80 files" });
    expect(t.toasts[0]).toMatchObject({ progress: 0.8, detail: "80 files" });
    t.dismiss(id);
    expect(t.toasts).toHaveLength(0);
  });

  it("honours an explicit ttl, including 0 for sticky", () => {
    const t = useToastsStore();
    t.push("info", "quick", { ttl: 100 });
    t.push("info", "sticky", { ttl: 0 });
    vi.advanceTimersByTime(200);
    expect(t.toasts.map((x) => x.title)).toEqual(["sticky"]);
  });

  it("caps the stack at six, dropping the oldest", () => {
    const t = useToastsStore();
    for (let i = 0; i < 9; i++) t.push("info", `t${i}`, { ttl: 0 });
    expect(t.toasts).toHaveLength(6);
    expect(t.toasts[0].title).toBe("t3");
  });

  it("ignores updates and dismisses for unknown ids", () => {
    const t = useToastsStore();
    t.push("info", "x", { ttl: 0 });
    t.update(999, { title: "nope" });
    t.dismiss(999);
    expect(t.toasts.map((x) => x.title)).toEqual(["x"]);
  });
});
