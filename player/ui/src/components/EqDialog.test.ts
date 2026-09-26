import { beforeEach, describe, expect, it } from "vitest";
import { bandResponseDb, bandSeverity, constrainBand, EQ_LIMITS, maxFreqFor, totalResponseDb } from "../eqResponse";
import { makeState } from "../test/fixtures";
import { $$, mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import EqDialog from "./EqDialog.vue";

async function boot(path: "pcm-shared" | "dop-exclusive" | "pcm-exclusive" = "pcm-shared", rate: number | null = null) {
  tauri.on("get_state", makeState({ status: "playing", output_path: path, output_rate_hz: rate }));
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

describe("EqDialog OK / Cancel", () => {
  const click = async (id: string) => {
    (document.body.querySelector(`[data-testid="${id}"]`) as HTMLElement).click();
    await settle();
  };

  it("OK keeps the edits", async () => {
    const { wrapper, dsp } = await boot();
    await dsp.applyPreset("builtin:rock");
    await click("eq-ok");
    expect(dsp.activePreset?.name).toBe("Rock");
    expect(wrapper.emitted("update:open")?.at(-1)).toEqual([false]);
  });

  it("Cancel puts back the bands and on/off state from when it opened", async () => {
    tauri.on("get_state", makeState({ status: "playing", output_path: "pcm-shared" }));
    const { wrapper } = mountApp(EqDialog, { open: false });
    const dsp = useDspStore();
    await settle();
    await dsp.applyPreset("builtin:jazz");
    await wrapper.setProps({ open: true });
    await settle();
    await dsp.applyPreset("builtin:rock");
    await dsp.saveEqEnabled(false);
    await click("eq-cancel");
    expect(dsp.activePreset?.name).toBe("Jazz");
    expect(dsp.eqEnabled).toBe(true);
    expect(tauri.callsTo("set_eq_enabled").at(-1)).toEqual({ enabled: true });
    expect(wrapper.emitted("update:open")?.at(-1)).toEqual([false]);
  });

  it("has the preset picker, Save as preset and OK/Cancel in one dialog", async () => {
    await boot();
    for (const sel of ['[aria-label="EQ preset"]', '[data-testid="save-preset"]', '[data-testid="eq-ok"]', '[data-testid="eq-cancel"]']) {
      expect(document.body.querySelector(sel)).not.toBeNull();
    }
  });
});

describe("EQ guardrails", () => {
  it("limits bands to the usable range for the output rate", () => {
    expect(maxFreqFor(44100)).toBe(19845);
    expect(maxFreqFor(192000)).toBe(EQ_LIMITS.freqMax);
    const c = constrainBand({ band_type: "peaking", freq: 24000, gain_db: 40, q: 99 }, 44100);
    expect(c).toEqual({ band_type: "peaking", freq: 19845, gain_db: 18, q: 18 });
    expect(constrainBand({ band_type: "low_shelf", freq: 5, gain_db: -40, q: 12 }, 48000)).toEqual({ band_type: "low_shelf", freq: 20, gain_db: -18, q: 3 });
    expect(constrainBand({ band_type: "high_pass", freq: 100, gain_db: 9, q: 1 }, 48000).gain_db).toBe(0);
    expect(constrainBand({ band_type: "peaking", freq: NaN, gain_db: NaN, q: NaN }, 48000)).toEqual({ band_type: "peaking", freq: 1000, gain_db: 0, q: 1 });
  });

  it("draws the curve for the output rate: a band above the cap is drawn at the cap", () => {
    const hi = { band_type: "high_shelf" as const, freq: 24000, gain_db: 6, q: 0.7 };
    const cap = { ...hi, freq: 19845 };
    expect(bandResponseDb(hi, 10000, 44100)).toBeCloseTo(bandResponseDb(cap, 10000, 44100), 6);
  });

  it("shows which rate the curve is for", async () => {
    await boot("pcm-shared", 44100);
    expect(document.body.querySelector('[data-testid="eq-rate"]')!.textContent).toContain("44.1 kHz");
    expect(document.body.querySelector('[data-testid="eq-rate"]')!.textContent).toContain("19845 Hz");
  });

  it("says so when nothing is playing and the curve falls back to 48 kHz", async () => {
    await boot("pcm-shared", null);
    expect(document.body.querySelector('[data-testid="eq-rate"]')!.textContent).toContain("48 kHz");
    expect(document.body.querySelector('[data-testid="eq-rate"]')!.textContent).toContain("nothing playing");
  });

  it("keeps dragged and typed values inside the limits", async () => {
    const { dsp } = await boot("pcm-shared", 44100);
    dsp.addBand();
    await settle();
    const node = () => nodes()[0];
    for (let i = 0; i < 400; i++) node().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", shiftKey: true, bubbles: true, cancelable: true }));
    for (let i = 0; i < 100; i++) node().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", shiftKey: true, bubbles: true, cancelable: true }));
    await settle();
    expect(dsp.rows[0].freq).toBeLessThanOrEqual(19845);
    expect(dsp.rows[0].gain_db).toBeLessThanOrEqual(18);
    node().dispatchEvent(new Event("focus"));
    await settle();
    const input = document.body.querySelector('[data-testid="eq-band-panel"] input[max="19845"]') as HTMLInputElement;
    input.value = "24000";
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(dsp.rows[0].freq).toBe(19845);
    expect(input.value).toBe("19845");
  });

  it("warns when the combined boost can clip, and not for gentle or cut-only settings", async () => {
    const { dsp } = await boot();
    await dsp.applyPreset("builtin:flat");
    await settle();
    expect(document.body.querySelector('[data-testid="eq-headroom"]')).toBeNull();
    dsp.addBand();
    await settle();
    dsp.updateRow(0, { gain_db: -6 });
    await settle();
    expect(document.body.querySelector('[data-testid="eq-headroom"]')).toBeNull(); // cut-only: no boost
    dsp.updateRow(0, { gain_db: 9 });
    await settle();
    expect(document.body.querySelector('[data-testid="eq-headroom"]')!.textContent).toContain("can clip");
    expect(totalResponseDb(dsp.activeBands, 1000)).toBeGreaterThan(0.5);
  });
});

describe("EQ point severity colours", () => {
  const peak = (gain_db: number, freq = 1000, q = 1) => ({ band_type: "peaking" as const, freq, gain_db, q });

  it("rates bands: fine, warning (yellow), bad (red)", () => {
    expect(bandSeverity(peak(3), 48000, 3).level).toBe("ok");
    expect(bandSeverity(peak(4), 48000, 5).level).toBe("ok"); // the usual preset boosts stay quiet
    expect(bandSeverity(peak(-12), 48000, 0).level).toBe("ok"); // cuts are safe
    expect(bandSeverity(peak(7), 48000, 7).level).toBe("warn");
    expect(bandSeverity(peak(2), 48000, 6.5).level).toBe("warn"); // combined boost may clip
    expect(bandSeverity(peak(16), 48000, 5).level).toBe("bad"); // one huge boost
    expect(bandSeverity(peak(2), 48000, 10.5).level).toBe("bad"); // combined boost will clip
    expect(bandSeverity(peak(0, 1000, 15), 48000, 0).level).toBe("warn"); // very narrow
    expect(bandSeverity(peak(0, 21000), 44100, 0).level).toBe("bad"); // above the cap
    expect(bandSeverity(peak(0, 18500), 44100, 0).level).toBe("warn"); // near the cap
    expect(bandSeverity(peak(0, 18500), 44100, 0).reason).toMatch(/top of the usable range/);
  });

  it("colours the graph's points and says why, not by colour alone", async () => {
    const { dsp } = await boot();
    dsp.addBand();
    await settle();
    const node = () => nodes()[0];
    expect(node().getAttribute("data-severity")).toBe("ok");
    expect(node().getAttribute("class")).toContain("fill-accent");
    dsp.updateRow(0, { gain_db: 8 });
    await settle();
    expect(node().getAttribute("data-severity")).toBe("warn");
    expect(node().getAttribute("class")).toContain("fill-warn");
    dsp.updateRow(0, { gain_db: 14 });
    await settle();
    expect(node().getAttribute("data-severity")).toBe("bad");
    expect(node().getAttribute("class")).toContain("fill-danger");
    expect(node().getAttribute("aria-label")).toContain("Bad:");
    node().dispatchEvent(new Event("focus"));
    await settle();
    const note = document.body.querySelector('[data-testid="eq-band-severity"]')!;
    expect(note.getAttribute("data-severity")).toBe("bad");
    expect(note.textContent).toContain("Bad for sound quality");
  });

  it("a disabled band is grey, whatever its settings", async () => {
    const { dsp } = await boot();
    dsp.addBand();
    dsp.updateRow(0, { gain_db: 14 });
    dsp.toggleRow(0);
    await settle();
    expect(nodes()[0].getAttribute("class")).toContain("fill-faint");
  });
});
