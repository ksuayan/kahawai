import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";
import { tauri } from "../test/tauri-mock";
import { useDspStore, validateRow } from "./dsp";

const devices = [
  { name: "Built-in Output", is_default: true },
  { name: "FIIO K15 ", is_default: false },
  { name: "RØDE Connect System", is_default: false },
];

function bootTauri(chosen: string | null = null) {
  tauri
    .on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14 })
    .on("get_output_devices", devices)
    .on("dop_status", { supported_rates: [176400], exclusive_available: true })
    .on("get_output_device", chosen);
}

beforeEach(() => {
  setActivePinia(createPinia());
  localStorage.clear();
});

describe("output device selection", () => {
  it("loads the device list and the saved choice at startup", async () => {
    bootTauri("FIIO K15 ");
    const dsp = useDspStore();
    await dsp.init();
    expect(dsp.devices).toEqual(devices);
    expect(dsp.outputDevice).toBe("FIIO K15 ");
    expect(dsp.outputDeviceMissing).toBe(false);
  });

  it("defaults to the system default (null)", async () => {
    bootTauri(null);
    const dsp = useDspStore();
    await dsp.init();
    expect(dsp.outputDevice).toBeNull();
    expect(dsp.outputDeviceMissing).toBe(false);
  });

  it("chooses a device by its exact name and refreshes the DoP status", async () => {
    bootTauri();
    const dsp = useDspStore();
    await dsp.init();
    tauri.on("dop_status", { supported_rates: [], exclusive_available: true });
    await dsp.chooseOutputDevice("FIIO K15 ");
    expect(dsp.outputDevice).toBe("FIIO K15 ");
    expect(tauri.callsTo("set_output_device")).toEqual([{ name: "FIIO K15 " }]);
    expect(dsp.dop?.supported_rates).toEqual([]);
  });

  it("switches back to the system default with null", async () => {
    bootTauri("FIIO K15 ");
    const dsp = useDspStore();
    await dsp.init();
    await dsp.chooseOutputDevice(null);
    expect(dsp.outputDevice).toBeNull();
    expect(tauri.callsTo("set_output_device")).toEqual([{ name: null }]);
  });

  it("flags a saved device that is no longer connected", async () => {
    bootTauri("Old USB DAC");
    const dsp = useDspStore();
    await dsp.init();
    expect(dsp.outputDeviceMissing).toBe(true);
  });

  it("rescans devices (hot-plug) and clears the missing flag when it returns", async () => {
    bootTauri("Old USB DAC");
    const dsp = useDspStore();
    await dsp.init();
    expect(dsp.outputDeviceMissing).toBe(true);
    tauri.on("get_output_devices", [...devices, { name: "Old USB DAC", is_default: false }]);
    await dsp.refreshDevices();
    expect(dsp.devices).toHaveLength(4);
    expect(dsp.outputDeviceMissing).toBe(false);
  });
});

describe("EQ rows", () => {
  it("validates frequency, gain and Q ranges", () => {
    const ok = { band_type: "peaking", freq: 1000, gain_db: 3, q: 1, enabled: true } as const;
    expect(validateRow(ok)).toBeNull();
    expect(validateRow({ ...ok, freq: 5 })).toMatch(/Frequency/);
    expect(validateRow({ ...ok, freq: 30000 })).toMatch(/Frequency/);
    expect(validateRow({ ...ok, gain_db: 30 })).toMatch(/Gain/);
    expect(validateRow({ ...ok, q: 0 })).toMatch(/Q/);
    expect(validateRow({ ...ok, q: NaN })).toMatch(/Q/);
  });

  it("pushes only enabled bands to the core and caps at 12", async () => {
    bootTauri();
    const dsp = useDspStore();
    await dsp.init();
    dsp.addBand();
    dsp.addBand();
    dsp.toggleRow(0);
    await Promise.resolve();
    const last = tauri.callsTo("set_eq_bands").at(-1) as { bands: unknown[] };
    expect(last.bands).toHaveLength(1);
    for (let i = 0; i < 20; i++) dsp.addBand();
    expect(dsp.rows.length).toBe(12);
    expect(dsp.canAddBand).toBe(false);
  });
});

describe("EQ preamp and profile import", () => {
  it("applies a preset's preamp and pushes it to the engine", async () => {
    bootTauri();
    const dsp = useDspStore();
    await dsp.init();
    await dsp.importProfile({ bands: [{ band_type: "peaking", freq: 1000, gain_db: 4, q: 1 }], preamp_db: -4.5 });
    expect(dsp.eqPreamp).toBe(-4.5);
    expect(tauri.callsTo("set_eq_preamp").at(-1)).toEqual({ db: -4.5 });
    dsp.saveUserPreset("Mine");
    expect(dsp.userPresets[0].preamp_db).toBe(-4.5);
    await dsp.applyPreset("builtin:flat");
    expect(dsp.eqPreamp).toBe(0);
    await dsp.applyPreset("user:Mine");
    expect(dsp.eqPreamp).toBe(-4.5);
    expect(dsp.activePreset?.id).toBe("user:Mine");
  });

  it("clamps the preamp and restores it from a snapshot", async () => {
    bootTauri();
    const dsp = useDspStore();
    await dsp.init();
    const snap = dsp.snapshotEq();
    await dsp.saveEqPreamp(-99);
    expect(dsp.eqPreamp).toBe(-24);
    await dsp.restoreEq(snap);
    expect(dsp.eqPreamp).toBe(0);
  });
});
