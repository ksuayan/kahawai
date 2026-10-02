import { beforeEach, describe, expect, it } from "vitest";
import { BUILTIN_PRESETS } from "../eqPresets";
import { useDspStore } from "../stores/dsp";
import { mountApp, options, openSelect, pick, settle, $$ } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import EqControl from "./EqControl.vue";

async function boot() {
  const { wrapper } = mountApp(EqControl);
  const dsp = useDspStore();
  await settle();
  return { wrapper, dsp };
}
const open = async (w: Awaited<ReturnType<typeof boot>>["wrapper"]) => {
  await w.get('[data-testid="eq-button"]').trigger("click");
  await settle();
};
const body = () => document.body;

beforeEach(() => localStorage.clear());

describe("EqControl", () => {
  it("offers Flat, Classical, Jazz, Rock, Pop and Talk Show", () => {
    expect(BUILTIN_PRESETS.map((p) => p.name)).toEqual(["Flat", "Classical", "Jazz", "Rock", "Pop", "Spoken word", "Talk Show"]);
    for (const p of BUILTIN_PRESETS) expect(p.bands.length).toBeLessThanOrEqual(8);
  });

  it("applies a built-in preset to the engine and turns EQ on", async () => {
    const { wrapper, dsp } = await boot();
    dsp.eqEnabled = false;
    await open(wrapper);
    await openSelect(body().querySelector('[aria-label="EQ preset"]') as HTMLElement);
    pick(options().find((o) => o.textContent?.trim() === "Rock")!);
    await settle();
    const rock = BUILTIN_PRESETS.find((p) => p.name === "Rock")!;
    expect(tauri.callsTo("set_eq_bands").at(-1)).toEqual({ bands: rock.bands });
    expect(tauri.callsTo("set_eq_enabled").at(-1)).toEqual({ enabled: true });
    expect(dsp.activePreset?.name).toBe("Rock");
  });

  it("shows Custom when the tuning matches no preset, and saves it as a user preset", async () => {
    const { wrapper, dsp } = await boot();
    dsp.addBand();
    dsp.updateRow(0, { gain_db: 5 });
    await settle();
    expect(dsp.activePreset).toBeNull();
    await open(wrapper);
    expect(body().querySelector('[aria-label="EQ preset"]')!.textContent).toContain("Custom");
    (body().querySelector('[data-testid="save-preset"]') as HTMLElement).click();
    await settle();
    const input = body().querySelector('[aria-label="Preset name"], [role="dialog"] [placeholder="My tuning"]') as HTMLInputElement;
    input.value = "Bassy";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    (body().querySelector('[role="dialog"] [data-testid="submit"]') as HTMLElement).click();
    await settle();
    expect(dsp.userPresets.map((p) => p.name)).toEqual(["Bassy"]);
    expect(dsp.activePreset?.id).toBe("user:Bassy");
    expect(JSON.parse(localStorage.getItem("kahawai-player.eq-user-presets")!)[0].name).toBe("Bassy");
  });

  it("applies and deletes a user preset", async () => {
    const { dsp } = await boot();
    dsp.addBand();
    dsp.updateRow(0, { gain_db: 3 });
    dsp.saveUserPreset("Mine");
    await dsp.applyPreset("builtin:flat");
    expect(dsp.activeBands).toEqual([]);
    await dsp.applyPreset("user:Mine");
    expect(dsp.activeBands[0].gain_db).toBe(3);
    dsp.deleteUserPreset("user:Mine");
    expect(dsp.presets.some((p) => p.id === "user:Mine")).toBe(false);
  });

  it("overwrites a user preset saved under the same name", async () => {
    const { dsp } = await boot();
    dsp.addBand();
    dsp.saveUserPreset("A");
    dsp.updateRow(0, { gain_db: 6 });
    dsp.saveUserPreset("A");
    expect(dsp.userPresets).toHaveLength(1);
    expect(dsp.userPresets[0].bands[0].gain_db).toBe(6);
  });

  it("has an accessible EQ button that reflects on/off", async () => {
    const { wrapper, dsp } = await boot();
    const b = wrapper.get('[data-testid="eq-button"]');
    expect(b.attributes("aria-label")).toBe("Equalizer");
    dsp.eqEnabled = false;
    await settle();
    expect(b.attributes("aria-pressed")).toBeUndefined();
    expect($$('[data-testid="eq-popover"]').length).toBe(0);
  });
});

describe("EqControl opens the editor directly", () => {
  it("shows the graph dialog on click, with no intermediate popover", async () => {
    const { wrapper } = await boot();
    expect(document.body.querySelector('[data-testid="eq-graph"]')).toBeNull();
    await open(wrapper);
    expect(document.body.querySelector('[data-testid="eq-graph"]')).not.toBeNull();
    expect(document.body.querySelector('[data-testid="eq-popover"]')).toBeNull();
  });
});
