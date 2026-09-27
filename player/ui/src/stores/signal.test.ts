import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { tauri } from "../test/tauri-mock";
import { useSignalStore } from "./signal";

const live = { name: "FIIO K15 ", rate_hz: 96000, bit_depth: 32, float: false, exclusive: true };

describe("signal store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.on("output_live_state", live);
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("reads the device now and then every interval while watched", async () => {
    const s = useSignalStore();
    const stop = s.watch(1000);
    await vi.advanceTimersByTimeAsync(0);
    expect(s.live).toEqual(live);
    expect(tauri.callsTo("output_live_state")).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(3000);
    expect(tauri.callsTo("output_live_state")).toHaveLength(4);
    stop();
  });

  it("stops polling once nobody is watching, and is reference-counted", async () => {
    const s = useSignalStore();
    const a = s.watch(1000);
    const b = s.watch(1000);
    await vi.advanceTimersByTimeAsync(0);
    a();
    await vi.advanceTimersByTimeAsync(2000);
    const whileOneWatcher = tauri.callsTo("output_live_state").length;
    expect(whileOneWatcher).toBeGreaterThan(2);
    b();
    b(); // a second stop is harmless
    const after = tauri.callsTo("output_live_state").length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(tauri.callsTo("output_live_state")).toHaveLength(after);
  });

  it("clears the state when the OS reports no device", async () => {
    tauri.on("output_live_state", null);
    const s = useSignalStore();
    s.live = live;
    await s.refresh();
    expect(s.live).toBeNull();
  });
});
