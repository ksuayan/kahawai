import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { clampAnalog, DEFAULT_ANALOG_SETTINGS } from "../types";
import { tauri } from "../test/tauri-mock";
import { useAnalogStore } from "./analog";

const sent = () => tauri.callsTo("set_analog").map((c) => (c as { settings: unknown }).settings);

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
});

describe("analog A/B store", () => {
  it("starts with A dry (off) and B on, listening to A", () => {
    const s = useAnalogStore();
    expect(s.a.enabled).toBe(false);
    expect(s.b.enabled).toBe(true);
    expect(s.active).toBe("a");
  });

  it("switching to a slot sends that slot's settings to the engine", () => {
    const s = useAnalogStore();
    s.update("b", { flavour: "solid_state", drive: 0.8 }); // inactive: not sent
    expect(sent()).toHaveLength(0);
    s.select("b");
    expect(sent().at(-1)).toMatchObject({ enabled: true, flavour: "solid_state", drive: 0.8 });
    s.toggle();
    expect(sent().at(-1)).toMatchObject({ enabled: false });
    expect(s.active).toBe("a");
  });

  it("edits to the slot being heard go live; edits to the other slot do not", () => {
    const s = useAnalogStore();
    s.select("b");
    tauri.calls.length = 0;
    s.update("b", { mix: 0.9 });
    expect(sent()).toEqual([expect.objectContaining({ mix: 0.9, enabled: true })]);
    s.update("a", { mix: 0.1 });
    expect(sent()).toHaveLength(1);
  });

  it("clamps values and ignores nonsense", () => {
    const s = useAnalogStore();
    s.update("a", { drive: 7, mix: -2, output_db: 99, sag: 4, transformer: -1 });
    expect([s.a.drive, s.a.mix, s.a.output_db, s.a.sag, s.a.transformer]).toEqual([1, 0, 6, 1, 0]);
    const c = clampAnalog({ ...DEFAULT_ANALOG_SETTINGS, drive: NaN, flavour: "nope" as never, antialias: "x9" as never });
    expect([c.drive, c.flavour, c.antialias]).toEqual([0.4, "warm_triode", "auto"]);
  });

  it("copies one slot over the other", () => {
    const s = useAnalogStore();
    s.update("b", { flavour: "solid_state", antialias: "x2_adaa" });
    s.copy("b", "a");
    expect(s.a).toMatchObject({ flavour: "solid_state", antialias: "x2_adaa", enabled: true });
    s.copy("a", "a"); // no-op
  });

  it("remembers the pair and the active slot across launches", async () => {
    const s = useAnalogStore();
    s.update("b", { drive: 0.75 });
    s.select("b");
    setActivePinia(createPinia());
    const again = useAnalogStore();
    await again.init();
    expect(again.b.drive).toBe(0.75);
    expect(again.active).toBe("b");
    expect(again.loaded).toBe(true);
  });

  it("with nothing saved, seeds B from the engine's settings when they are on", async () => {
    tauri.on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14, analog: { ...DEFAULT_ANALOG_SETTINGS, enabled: true, flavour: "solid_state", drive: 0.6 } });
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const s = useAnalogStore();
    await s.init();
    expect(s.b).toMatchObject({ flavour: "solid_state", drive: 0.6 });
    expect(s.active).toBe("b");
  });
});
