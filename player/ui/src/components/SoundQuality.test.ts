import { describe, expect, it } from "vitest";
import { makeState } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import { useSettingsStore } from "../stores/settings";
import type { DeviceCapabilities } from "../types";
import DeviceCapabilitiesPanel from "./DeviceCapabilities.vue";
import SoundQualitySection from "./SoundQualitySection.vue";

const k15: DeviceCapabilities = {
  name: "FIIO K15 ",
  transport: "usb",
  external_dac: true,
  sample_rates: [44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000],
  bit_depths: [16, 32],
  float32: false,
  dop_rates: [176400, 352800, 705600],
  exclusive_available: true,
};
const speakers: DeviceCapabilities = {
  name: "MacBook Pro Speakers",
  transport: "built-in",
  external_dac: false,
  sample_rates: [44100, 48000],
  bit_depths: [32],
  float32: true,
  dop_rates: [],
  exclusive_available: true,
};

describe("DeviceCapabilities", () => {
  it("shows every standard rate, dimming the ones the device doesn't offer", () => {
    const { wrapper } = mountApp(DeviceCapabilitiesPanel, { caps: speakers });
    const rates = wrapper.findAll('[data-testid="cap-rates"] span[data-ok]');
    expect(rates.map((r) => r.text())).toEqual(["44.1", "48", "88.2", "96", "176.4", "192", "352.8", "384", "705.6", "768"]);
    expect(rates.filter((r) => r.attributes("data-ok") === "true").map((r) => r.text())).toEqual(["44.1", "48"]);
  });

  it("names the connection and whether it is an external DAC", () => {
    const usb = mountApp(DeviceCapabilitiesPanel, { caps: k15, knownDsd: true }).wrapper;
    expect(usb.get('[data-testid="cap-transport"]').text()).toBe("USB");
    expect(usb.get('[data-testid="cap-external"]').text()).toBe("External DAC");
    const built = mountApp(DeviceCapabilitiesPanel, { caps: speakers }).wrapper;
    expect(built.get('[data-testid="cap-transport"]').text()).toBe("Built-in");
    expect(built.get('[data-testid="cap-external"]').text()).toBe("Not an external DAC");
  });

  it("counts 24-bit as available through a 32-bit slot, and marks DSD support", () => {
    const w = mountApp(DeviceCapabilitiesPanel, { caps: k15, knownDsd: true }).wrapper;
    const depth = (d: number) => w.findAll('[data-testid="cap-depths"] span').find((s) => s.text().startsWith(`${d}-bit`))!;
    expect(depth(16).attributes("data-ok")).toBe("true");
    expect(depth(24).attributes("data-ok")).toBe("true");
    expect(depth(24).text()).toContain("32-bit slot");
    const probe = w.get('[data-testid="dsd-rate-probe"]').text();
    expect(probe).toContain("DSD64 ✓");
    expect(probe).toContain("DSD256 ✓");
    expect(probe).toContain("DSD512 ✗");
    expect(w.get('[data-testid="cap-dsd-known"]').text()).toBe("Known to decode DoP");
  });
});

async function mountSection(caps: DeviceCapabilities | null, state?: ReturnType<typeof makeState>) {
  const { wrapper } = mountApp(SoundQualitySection, {}, {}, () => {
    if (caps) useDspStore().dop = { supported_rates: caps.dop_rates, exclusive_available: true, capabilities: caps };
    if (state) usePlayerStore().raw = state;
  });
  await settle();
  return wrapper;
}

describe("Sound quality", () => {
  it("offers Best quality and Compatible and saves the choice through the core", async () => {
    const w = await mountSection(k15);
    expect(w.get('[data-testid="quality-best"]').attributes("aria-checked")).toBe("true");
    await w.get('[data-testid="quality-compatible"]').trigger("click");
    await settle();
    expect(tauri.callsTo("set_quality_mode")).toEqual([{ mode: "compatible" }]);
    expect(useSettingsStore().qualityMode).toBe("compatible");
    expect(w.get('[data-testid="quality-status"]').text()).toContain("Shared output");
  });

  it("says Best quality is ready on an external DAC", async () => {
    const w = await mountSection(k15);
    expect(w.get('[data-testid="quality-status"]').text()).toContain("Ready");
  });

  it("explains why it stays on shared output for a built-in device", async () => {
    const w = await mountSection(speakers);
    expect(w.get('[data-testid="quality-status"]').text()).toContain("isn't an external DAC");
  });

  it("names the processing that is holding Best quality back", async () => {
    const w = await mountSection(k15, makeState({ exclusive_blockers: ["EQ", "Volume"] }));
    expect(w.get('[data-testid="quality-status"]').text()).toContain("Paused: EQ and Volume are on");
    expect(w.get('[data-testid="processing-eq"]').attributes("data-state")).toBe("on");
    expect(w.get('[data-testid="processing-volume"]').attributes("data-state")).toBe("on");
    expect(w.get('[data-testid="processing-loudness"]').attributes("data-state")).toBe("off");
  });

  it("shows EQ, loudness, analog and volume as bypassed while exclusive output plays", async () => {
    const w = await mountSection(
      k15,
      makeState({ status: "playing", output_path: "pcm-exclusive", output_rate_hz: 96000, exclusive_blockers: [] }),
    );
    expect(w.get('[data-testid="quality-status"]').text()).toContain("bit-perfect at 96 kHz");
    for (const n of ["eq", "loudness", "analog", "volume"]) {
      expect(w.get(`[data-testid="processing-${n}"]`).attributes("data-state")).toBe("bypassed");
    }
  });
});
