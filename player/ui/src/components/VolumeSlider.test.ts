import { beforeEach, describe, expect, it, vi } from "vitest";
import { makeState } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "../stores/player";
import VolumeSlider from "./VolumeSlider.vue";

async function boot(volume = 0.7, extra: Record<string, unknown> = {}) {
  tauri.on("get_state", makeState({ volume, ...extra }));
  const { wrapper } = mountApp(VolumeSlider);
  await usePlayerStore().init();
  await settle();
  return wrapper;
}
const thumb = (w: Awaited<ReturnType<typeof boot>>) => w.find('[role="slider"]');

beforeEach(() => vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] }));

describe("VolumeSlider", () => {
  it("shows the engine volume as a percentage", async () => {
    // Mounted before the store hydrates, then follows the engine.
    const w = await boot(0.7);
    expect(thumb(w).attributes("aria-valuenow")).toBe("70");
    expect(thumb(w).attributes("aria-label")).toBe("Volume");
    expect(thumb(w).attributes("aria-valuemax")).toBe("100");
  });

  it("sends every step to the engine live, as 0..1", async () => {
    const w = await boot(0.5);
    await thumb(w).trigger("keydown", { key: "ArrowRight" });
    await thumb(w).trigger("keydown", { key: "ArrowRight" });
    await settle();
    expect(tauri.callsTo("set_volume")).toEqual([{ v: 0.51 }, { v: 0.52 }]);
    expect(thumb(w).attributes("aria-valuenow")).toBe("52");
  });

  it("clamps at 0 and 100", async () => {
    const w = await boot(1);
    await thumb(w).trigger("keydown", { key: "ArrowRight" });
    await settle();
    expect(thumb(w).attributes("aria-valuenow")).toBe("100");
    await thumb(w).trigger("keydown", { key: "Home" });
    await settle();
    expect(tauri.callsTo("set_volume").at(-1)).toEqual({ v: 0 });
  });

  it("does not let an in-flight event that predates the latest input pull the thumb back", async () => {
    const w = await boot(0.5);
    await thumb(w).trigger("keydown", { key: "End" }); // user goes to 100
    await settle();
    tauri.emit("player-state", makeState({ volume: 0.5 })); // stale echo
    await settle();
    expect(thumb(w).attributes("aria-valuenow")).toBe("100");

    // After the guard window, the engine is authoritative again.
    vi.advanceTimersByTime(700);
    tauri.emit("player-state", makeState({ volume: 0.3 }));
    await settle();
    expect(thumb(w).attributes("aria-valuenow")).toBe("30");
  });

  it("follows volume changes made elsewhere (e.g. the keyboard shortcut)", async () => {
    const w = await boot(0.5);
    tauri.emit("player-state", makeState({ volume: 0.8 }));
    await settle();
    expect(thumb(w).attributes("aria-valuenow")).toBe("80");
  });

  it("dims itself and explains why on exclusive DoP output", async () => {
    const w = await boot(0.5, { output_path: "dop-exclusive" });
    expect(w.classes()).toContain("opacity-40");
    expect(w.attributes("title")).toMatch(/ignored on exclusive DoP/);
    const normal = await boot(0.5);
    expect(normal.classes()).not.toContain("opacity-40");
  });

  it("is dimmed on bit-perfect output and points the user at the DAC's own volume", async () => {
    const w = await boot(0.5, { output_path: "pcm-exclusive" });
    expect(w.classes()).toContain("opacity-40");
    expect(w.attributes("title")).toMatch(/DAC's volume/);
    const dop = await boot(0.5, { output_path: "dop-exclusive" });
    expect(dop.attributes("title")).toMatch(/ignored on exclusive DoP/);
    const shared = await boot(0.5, { output_path: "pcm-shared" });
    expect(shared.classes()).not.toContain("opacity-40");
    expect(shared.attributes("title")).toBe("Volume");
  });
});
