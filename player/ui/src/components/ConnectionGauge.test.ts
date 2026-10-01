import { beforeEach, describe, expect, it, vi } from "vitest";
import { makeState, makeTrack } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "../stores/player";
import ConnectionGauge from "./ConnectionGauge.vue";

async function boot(over: Record<string, unknown> = {}) {
  tauri.on("get_state", makeState({ status: "playing", track: makeTrack({ duration_ms: 200_000 }), ...over }));
  const { wrapper } = mountApp(ConnectionGauge);
  await usePlayerStore().init();
  await settle();
  return wrapper;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
});

describe("ConnectionGauge", () => {
  it("shows the speed and the seconds buffered ahead", async () => {
    const w = await boot({ download_bps: 4_375_000, buffer_ahead_ms: 18_400 });
    const g = w.get('[data-testid="connection-gauge"]');
    expect(w.get('[data-testid="connection-rate"]').text()).toBe("35.0 Mbit/s");
    expect(w.get('[data-testid="connection-ahead"]').text()).toBe("18 s");
    expect(g.attributes("data-health")).toBe("good");
    expect(g.attributes("aria-label")).toBe("Connection: 35.0 Mbit/s · 18 s buffered ahead");
  });

  it("fills the bar in proportion to the buffer and colours it by health", async () => {
    const fair = await boot({ download_bps: 500_000, buffer_ahead_ms: 6000 });
    expect(fair.get('[data-testid="connection-gauge"]').attributes("data-health")).toBe("fair");
    expect(fair.get('[data-testid="connection-fill"]').attributes("style")).toContain("width: 20%");
    expect(fair.get('[data-testid="connection-fill"]').classes()).toContain("bg-warn");

    const low = await boot({ download_bps: 20_000, buffer_ahead_ms: 1200 });
    expect(low.get('[data-testid="connection-gauge"]').attributes("data-health")).toBe("low");
    expect(low.get('[data-testid="connection-fill"]').classes()).toContain("bg-danger");
  });

  it("says measuring until a speed is known and still shows the buffer", async () => {
    const w = await boot({ download_bps: null, buffer_ahead_ms: 12_000 });
    expect(w.get('[data-testid="connection-rate"]').text()).toBe("—");
    expect(w.get('[data-testid="connection-gauge"]').attributes("aria-label")).toContain("measuring");
  });

  it("stays green and full when the whole track is already fetched, however little is left", async () => {
    const w = await boot({ download_bps: 9_000_000, buffer_ahead_ms: 800, buffer_complete: true });
    expect(w.get('[data-testid="connection-gauge"]').attributes("data-health")).toBe("good");
    expect(w.get('[data-testid="connection-ahead"]').text()).toBe("all");
    expect(w.get('[data-testid="connection-fill"]').attributes("style")).toContain("width: 100%");
    expect(w.get('[data-testid="connection-gauge"]').attributes("aria-label")).toContain("whole track buffered");
  });

  it("is hidden when nothing is streaming or nothing is known", async () => {
    expect((await boot({ status: "stopped", download_bps: 1_000_000, buffer_ahead_ms: 9000 })).find('[data-testid="connection-gauge"]').exists()).toBe(false);
    expect((await boot({ download_bps: null, buffer_ahead_ms: null })).find('[data-testid="connection-gauge"]').exists()).toBe(false);
    expect((await boot({ track: null, download_bps: 1, buffer_ahead_ms: 1 })).find('[data-testid="connection-gauge"]').exists()).toBe(false);
  });

  it("follows live engine updates", async () => {
    const w = await boot({ download_bps: 4_000_000, buffer_ahead_ms: 20_000 });
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ duration_ms: 200_000 }), download_bps: 300_000, buffer_ahead_ms: 2000 }));
    await settle();
    expect(w.get('[data-testid="connection-gauge"]').attributes("data-health")).toBe("low");
    expect(w.get('[data-testid="connection-rate"]').text()).toBe("2.4 Mbit/s");
  });

  it("says buffering, red and empty, when playback has run out of audio and is waiting on the network", async () => {
    const w = await boot({ download_bps: 4_000_000, buffer_ahead_ms: 0, buffering: true });
    const g = w.get('[data-testid="connection-gauge"]');
    expect(g.attributes("data-health")).toBe("low");
    expect(w.get('[data-testid="connection-rate"]').text()).toBe("Buffering…");
    expect(w.get('[data-testid="connection-fill"]').attributes("style")).toContain("width: 0%");
    expect(g.attributes("aria-label")).toBe("Connection: buffering, waiting for the server");
  });

  it("still shows while buffering even if no speed or buffer figure is known", async () => {
    const w = await boot({ download_bps: null, buffer_ahead_ms: null, buffering: true });
    expect(w.find('[data-testid="connection-gauge"]').exists()).toBe(true);
  });
});
