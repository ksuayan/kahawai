import { beforeEach, describe, expect, it } from "vitest";
import { bandResponseDb, totalResponseDb } from "../eqResponse";
import { makeState } from "../test/fixtures";
import { $$, mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import EqDialog from "./EqDialog.vue";

async function boot(path: "pcm-shared" | "dop-exclusive" | "pcm-exclusive" = "pcm-shared") {
  tauri.on("get_state", makeState({ status: "playing", output_path: path }));
  const { wrapper } = mountApp(EqDialog, { open: true });
  await usePlayerStore().init();
  const dsp = useDspStore();
  await settle();
  return { wrapper, dsp };
}
const nodes = () => $$('[data-testid="eq-node"]');
const curve = () => document.body.querySelector('[data-testid="eq-curve"]')!.getAttribute("d")!;

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML = "";
});

describe("EQ response maths", () => {
  it("peaks at the band's gain at its centre and is flat far away", () => {
    const b = { band_type: "peaking" as const, freq: 1000, gain_db: 6, q: 1 };
    expect(bandResponseDb(b, 1000)).toBeCloseTo(6, 1);
    expect(Math.abs(bandResponseDb(b, 30))).toBeLessThan(0.3);
  });
  it("shelves and passes behave", () => {
    expect(bandResponseDb({ band_type: "low_shelf", freq: 200, gain_db: 6, q: 0.7 }, 30)).toBeCloseTo(6, 0);
    expect(bandResponseDb({ band_type: "high_shelf", freq: 4000, gain_db: -6, q: 0.7 }, 18000)).toBeCloseTo(-6, 0);
    expect(bandResponseDb({ band_type: "high_pass", freq: 100, gain_db: 0, q: 0.7 }, 20)).toBeLessThan(-20);
    expect(totalResponseDb([], 1000)).toBe(0);
  });
});

describe("EqDialog", () => {
  it("draws one node per band and a curve that follows the preset", async () => {
    const { dsp } = await boot();
    const flat = curve();
    await dsp.applyPreset("builtin:rock");
    await settle();
    expect(nodes()).toHaveLength(4);
    expect(curve()).not.toBe(flat);
  });

  it("moves a band from the keyboard (gain and frequency) and pushes it to the engine", async () => {
    const { dsp } = await boot();
    await dsp.applyPreset("builtin:jazz");
    await settle();
    const before = { ...dsp.rows[1] };
    nodes()[1].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }));
    nodes()[1].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    await settle();
    expect(dsp.rows[1].gain_db).toBe(before.gain_db + 0.5);
    expect(dsp.rows[1].freq).toBeGreaterThan(before.freq);
    expect(tauri.callsTo("set_eq_bands").length).toBeGreaterThan(0);
    expect(dsp.activePreset).toBeNull(); // now a custom tuning
  });

  it("lists built-in presets in its picker and offers Save as preset", async () => {
    await boot();
    expect(document.body.querySelector('[aria-label="EQ preset"]')).not.toBeNull();
    expect(document.body.querySelector('[data-testid="save-preset"]')).not.toBeNull();
  });

  it("is fully usable and shows no warning on the shared PCM path", async () => {
    await boot("pcm-shared");
    expect(document.body.querySelector('[data-testid="eq-unsupported"]')).toBeNull();
    expect(document.body.querySelector('[data-testid="eq-editor"]')!.hasAttribute("inert")).toBe(false);
  });

  it.each(["dop-exclusive", "pcm-exclusive"] as const)("is dimmed with an explanation on %s", async (path) => {
    const { dsp } = await boot(path);
    expect(document.body.querySelector('[data-testid="eq-unsupported"]')!.textContent).toContain("EQ is not supported for this stream type.");
    const editor = document.body.querySelector('[data-testid="eq-editor"]')!;
    expect(editor.className).toContain("opacity-40");
    expect(editor.hasAttribute("inert")).toBe(true);
    // ...and edits are refused even if something dispatches them.
    await dsp.applyPreset("builtin:rock");
    await settle();
    nodes()[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }));
    expect(dsp.rows[0].gain_db).toBe(4);
  });
});
