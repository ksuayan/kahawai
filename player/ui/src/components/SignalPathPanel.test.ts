import { describe, expect, it } from "vitest";
import { makeState, makeTrack } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import { useSignalStore } from "../stores/signal";
import type { PlayerState } from "../types";
import type { OutputLive } from "../tauri";
import SignalPathPanel from "./SignalPathPanel.vue";

const k15 = (over: Partial<OutputLive> = {}): OutputLive => ({
  name: "FIIO K15 ",
  rate_hz: 44100,
  bit_depth: 32,
  float: false,
  exclusive: false,
  ...over,
});

async function mountPanel(state: PlayerState, live: OutputLive | null) {
  const { wrapper } = mountApp(SignalPathPanel, {}, {}, () => {
    usePlayerStore().raw = state;
    useSignalStore().live = live;
    useDspStore().dop = {
      supported_rates: [],
      exclusive_available: true,
      capabilities: {
        name: "FIIO K15 ",
        transport: "usb",
        external_dac: true,
        sample_rates: [44100],
        bit_depths: [16, 32],
        float32: false,
        dop_rates: [],
        exclusive_available: true,
      },
    };
  });
  await settle();
  return wrapper;
}
const text = (w: Awaited<ReturnType<typeof mountPanel>>, id: string) => w.get(`[data-testid="${id}"]`).text();

describe("SignalPathPanel", () => {
  it("shows the file and the device side by side, with the device name and connection", async () => {
    const t = makeTrack({ format: "flac", bit_depth: 24, sample_rate: 96000 });
    const w = await mountPanel(makeState({ status: "playing", track: t, output_path: "pcm-exclusive", format: "passthrough" }), k15({ rate_hz: 96000, exclusive: true }));
    expect(text(w, "sp-file-depth")).toContain("24-bit");
    expect(text(w, "sp-file-rate")).toContain("96 kHz");
    expect(text(w, "sp-file-format")).toContain("FLAC · 24/96k → PASSTHROUGH → Bit-perfect · exclusive");
    expect(text(w, "sp-out-rate")).toContain("96 kHz");
    expect(text(w, "sp-out-depth")).toContain("32-bit slot · 24-bit audio");
    expect(text(w, "sp-out-mode")).toContain("PCM · bit-perfect · exclusive");
    expect(text(w, "sp-device")).toContain("FIIO K15 · usb");
    expect(w.find('[data-testid="sp-exclusive"]').exists()).toBe(true);
  });

  it("marks a matched rate and a lossless depth", async () => {
    const t = makeTrack({ format: "flac", bit_depth: 16, sample_rate: 44100 });
    const w = await mountPanel(makeState({ status: "playing", track: t, output_path: "pcm-exclusive" }), k15({ exclusive: true }));
    expect(w.get('[data-testid="sp-rate-link"]').attributes("data-state")).toBe("match");
    expect(text(w, "sp-rate-link")).toContain("matched");
    expect(w.get('[data-testid="sp-depth-link"]').attributes("data-state")).toBe("match");
  });

  it("flags a resampled rate when the device runs at something else", async () => {
    const t = makeTrack({ format: "mp3", bit_depth: null, sample_rate: 48000 });
    const w = await mountPanel(makeState({ status: "playing", track: t, output_path: "pcm-shared" }), k15({ rate_hz: 44100 }));
    expect(w.get('[data-testid="sp-rate-link"]').attributes("data-state")).toBe("convert");
    expect(text(w, "sp-rate-link")).toContain("resampled");
    expect(text(w, "sp-out-mode")).toContain("PCM · shared mixer");
  });

  it("shows native DSD as carried over DoP", async () => {
    const t = makeTrack({ format: "dsf", bit_depth: 1, sample_rate: 2822400 });
    const w = await mountPanel(makeState({ status: "playing", track: t, output_path: "dop-exclusive", format: "dop" }), k15({ rate_hz: 176400, exclusive: true }));
    expect(text(w, "sp-file-depth")).toContain("1-bit (DSD)");
    expect(text(w, "sp-file-rate")).toContain("DSD64");
    expect(text(w, "sp-file-rate")).toContain("2.8224 MHz");
    expect(text(w, "sp-out-rate")).toContain("176.4 kHz");
    expect(text(w, "sp-out-mode")).toContain("DSD over PCM (DoP)");
    expect(w.get('[data-testid="sp-rate-link"]').attributes("data-state")).toBe("carried");
  });

  it("shows DSD converted to PCM as converted", async () => {
    const t = makeTrack({ format: "dsf", bit_depth: 1, sample_rate: 2822400 });
    const w = await mountPanel(makeState({ status: "playing", track: t, output_path: "pcm-shared", format: "flac" }), k15({ rate_hz: 88200 }));
    expect(w.get('[data-testid="sp-rate-link"]').attributes("data-state")).toBe("convert");
    expect(text(w, "sp-file-format")).toContain("→ FLAC →");
  });

  it("labels an MQA file, and says the shared mixer doesn't decode it", async () => {
    const t = makeTrack({ format: "flac", bit_depth: 24, sample_rate: 48000, mqa: true, original_sample_rate: 96000 });
    const shared = await mountPanel(makeState({ status: "playing", track: t, output_path: "pcm-shared" }), k15({ rate_hz: 48000 }));
    expect(text(shared, "sp-file-title")).toContain("MQA");
    expect(text(shared, "sp-out-mode")).toContain("MQA not decoded");
    const bp = await mountPanel(makeState({ status: "playing", track: t, output_path: "pcm-exclusive" }), k15({ rate_hz: 48000, exclusive: true }));
    expect(text(bp, "sp-out-mode")).toContain("MQA stream, untouched");
  });

  it("shows the device's real state, and no comparison, when nothing is playing", async () => {
    const w = await mountPanel(makeState({ status: "stopped", track: null }), k15());
    expect(text(w, "sp-file-format")).toContain("Nothing playing");
    expect(text(w, "sp-out-rate")).toContain("44.1 kHz");
    expect(text(w, "sp-out-mode")).toContain("Idle");
    expect(w.get('[data-testid="sp-rate-link"]').attributes("data-state")).toBe("none");
  });
});
