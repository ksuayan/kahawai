import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ANALOG_FLAVOURS, ANTI_ALIAS_CHOICES, clampAnalog, DEFAULT_ANALOG_SETTINGS, FLAVOUR_INFO, LISTENING_RECIPES } from "../types";
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

describe("analog flavours", () => {
  it("has 21 flavours, each with a label, a blurb and typical sag/transformer", () => {
    expect(ANALOG_FLAVOURS).toHaveLength(21);
    expect(new Set(ANALOG_FLAVOURS).size).toBe(21);
    for (const f of ANALOG_FLAVOURS) {
      const i = FLAVOUR_INFO[f];
      expect(i.label && i.short && i.blurb).toBeTruthy();
      expect(i.sag).toBeGreaterThanOrEqual(0);
      expect(i.sag).toBeLessThanOrEqual(1);
      expect(i.transformer).toBeGreaterThanOrEqual(0);
      expect(i.transformer).toBeLessThanOrEqual(1);
    }
  });

  it("choosing a flavour also applies its typical sag and transformer, and goes live if it is the heard slot", () => {
    const s = useAnalogStore();
    s.select("b");
    tauri.calls.length = 0;
    s.setFlavour("b", "push_pull");
    expect(s.b).toMatchObject({ flavour: "push_pull", sag: 0.5, transformer: 0.5 });
    expect(sent().at(-1)).toMatchObject({ flavour: "push_pull", sag: 0.5, transformer: 0.5 });
    s.setFlavour("a", "hard_transistor");
    expect(s.a).toMatchObject({ flavour: "hard_transistor", sag: 0, transformer: 0 });
    expect(sent()).toHaveLength(1); // A is not heard
    s.setFlavour("a", "nope" as never); // ignored
    expect(s.a.flavour).toBe("hard_transistor");
  });

  it("falls back to the 12AX7 for an unknown flavour in saved settings", () => {
    expect(clampAnalog({ ...DEFAULT_ANALOG_SETTINGS, flavour: "tube_9999" as never }).flavour).toBe("warm_triode");
  });
});

describe("flavour list stays in step with the Rust core", () => {
  it("has exactly the flavours the core's round-trip test names", async () => {
    const { readFileSync } = await import("node:fs");
    const { join } = await import("node:path");
    const rust = readFileSync(join(__dirname, "../../../../crates/kahawai-player-core/src/analog.rs"), "utf8");
    const names = [...rust.matchAll(/\(AnalogFlavour::\w+, "([a-z0-9_]+)"\)/g)].map((m) => m[1]);
    expect(names.length).toBeGreaterThan(0);
    expect([...ANALOG_FLAVOURS].sort()).toEqual([...names].sort());
  });
});

describe("listening recipes", () => {
  it("loads both slots (with the flavours' typical values), starts on A, and sends A to the engine", () => {
    const s = useAnalogStore();
    const r = LISTENING_RECIPES.find((x) => x.id === "jfet-vs-300b")!;
    s.select("b");
    tauri.calls.length = 0;
    s.applyRecipe(r);
    expect(s.active).toBe("a");
    expect(s.a).toMatchObject({ enabled: true, flavour: "jfet", drive: 0.6, mix: 0.7, sag: 0, transformer: 0 });
    expect(s.b).toMatchObject({ enabled: true, flavour: "tube_300b", drive: 0.6, mix: 0.7, sag: 0.4, transformer: 0.5 });
    expect(sent().at(-1)).toMatchObject({ flavour: "jfet" });
  });

  it("'warmth against nothing' puts a dry signal in A", () => {
    const s = useAnalogStore();
    s.applyRecipe(LISTENING_RECIPES[0]);
    expect(s.a.enabled).toBe(false);
    expect(s.b.enabled).toBe(true);
    expect(sent().at(-1)).toMatchObject({ enabled: false });
  });

  it("every recipe is complete and uses only real flavours and plans", () => {
    expect(LISTENING_RECIPES.length).toBeGreaterThanOrEqual(8);
    expect(new Set(LISTENING_RECIPES.map((r) => r.id)).size).toBe(LISTENING_RECIPES.length);
    for (const r of LISTENING_RECIPES) {
      expect(r.title && r.idea && r.play && r.listen).toBeTruthy();
      for (const part of [r.a, r.b]) {
        if (part.flavour) expect(ANALOG_FLAVOURS).toContain(part.flavour);
        if (part.antialias) expect(ANTI_ALIAS_CHOICES).toContain(part.antialias);
      }
    }
  });
});

describe("the design document lists the recipes", () => {
  it("names every listening recipe in Analog-Emulation.md", async () => {
    const { readFileSync } = await import("node:fs");
    const { join } = await import("node:path");
    const doc = readFileSync(join(__dirname, "../../../../Analog-Emulation.md"), "utf8");
    for (const r of LISTENING_RECIPES) expect(doc).toContain(r.title);
    expect(doc).toMatch(/## 18\. Listening suggestions/);
    expect(doc.trimEnd().split("\n## ").pop()).toMatch(/^19\. References/); // References is the last section
  });
});

describe("level measurements and matching", () => {
  const level = (delta_db: number, seconds = 5, peak_dbfs = -6) => ({ input_lufs: -20, output_lufs: -20 + delta_db, delta_db, peak_dbfs, seconds });
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-26T12:00:00Z"));
  });
  afterEach(() => vi.useRealTimers());

  it("records the level change of the slot being heard, once enough audio is behind it", () => {
    const s = useAnalogStore();
    expect(s.measured).toEqual({ a: 0, b: null }); // A is dry: 0 by definition
    s.select("b");
    vi.advanceTimersByTime(10_000);
    s.noteLevel(level(1.8, 1.0)); // too little audio
    expect(s.measured.b).toBeNull();
    s.noteLevel(level(1.8));
    expect(s.measured.b).toBe(1.8);
    s.noteLevel(null);
    expect(s.measured.b).toBe(1.8);
  });

  it("ignores a reading that predates the last change of settings", () => {
    const s = useAnalogStore();
    s.select("b"); // sends settings to the engine "now"
    s.noteLevel(level(4, 10)); // 10 s of audio, but the settings changed just now: that is the old reading
    expect(s.measured.b).toBeNull();
    vi.advanceTimersByTime(11_000);
    s.noteLevel(level(4, 10));
    expect(s.measured.b).toBe(4);
  });

  it("forgets a slot's measurement when it is edited, and keeps a dry slot at 0", () => {
    const s = useAnalogStore();
    s.select("b");
    vi.advanceTimersByTime(10_000);
    s.noteLevel(level(2));
    s.update("b", { drive: 0.9 });
    expect(s.measured.b).toBeNull();
    s.update("b", { enabled: false });
    expect(s.measured.b).toBe(0);
    s.update("a", { enabled: true });
    expect(s.measured.a).toBeNull();
  });

  it("matches one slot to the other with the Output trim, in half-dB steps, and carries the measurement over", () => {
    const s = useAnalogStore();
    s.measured.b = 1.26; // B is 1.26 dB louder than A (dry, 0)
    const r = s.matchLevel("a", "b");
    expect(s.b.output_db).toBe(-1.5);
    expect(r).toEqual({ changed_db: -1.5, clamped: false });
    expect(s.measured.b).toBeCloseTo(-0.24, 5); // 1.26 - 1.5
    expect(s.matchLevel("a", "b")).toMatchObject({ changed_db: 0 }); // the 0.24 dB left is inside half a step: already as level as the slider allows
  });

  it("does not go beyond the slider, and says so; needs both slots measured", () => {
    const s = useAnalogStore();
    expect(s.matchLevel("a", "b")).toBeNull(); // B unmeasured
    s.measured.b = 9;
    expect(s.matchLevel("a", "b")).toEqual({ changed_db: -6, clamped: true });
    expect(s.b.output_db).toBe(-6);
  });

  it("a matched change goes live when that slot is the one being heard", () => {
    const s = useAnalogStore();
    s.select("b");
    tauri.calls.length = 0;
    s.measured.b = 2;
    s.matchLevel("a", "b");
    expect(sent().at(-1)).toMatchObject({ output_db: -2 });
  });
});

describe("restoring the pair at launch", () => {
  it("sends the restored active slot to the engine, so both agree even if the engine's file was reset", async () => {
    const s = useAnalogStore();
    s.update("b", { flavour: "tube_2a3", drive: 0.7 });
    s.select("b");
    setActivePinia(createPinia());
    tauri.calls.length = 0;
    const again = useAnalogStore();
    await again.init();
    expect(sent()).toHaveLength(1);
    expect(sent()[0]).toMatchObject({ flavour: "tube_2a3", drive: 0.7, enabled: true });
  });

  it("with nothing saved, sends nothing (the engine's own settings are the truth)", async () => {
    const s = useAnalogStore();
    await s.init();
    expect(sent()).toHaveLength(0);
  });
});
