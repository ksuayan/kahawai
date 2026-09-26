import { beforeEach, describe, expect, it, vi } from "vitest";
import { makeState, makeTrack } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "../stores/player";
import SeekBar from "./SeekBar.vue";

async function boot(state = makeState({ status: "paused", position_ms: 83_000, track: makeTrack({ duration_ms: 318_742 }) })) {
  tauri.on("get_state", state);
  const { wrapper } = mountApp(SeekBar);
  await usePlayerStore().init();
  await settle();
  return wrapper;
}
const thumb = (w: Awaited<ReturnType<typeof boot>>) => w.find('[role="slider"]');

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
});

describe("SeekBar", () => {
  it("shows elapsed and total time and puts the thumb at the playhead", async () => {
    const w = await boot();
    expect(w.get('[data-testid="elapsed"]').text()).toBe("1:23");
    expect(w.get('[data-testid="duration"]').text()).toBe("5:18");
    expect(thumb(w).attributes("aria-valuenow")).toBe("83000");
    expect(thumb(w).attributes("aria-valuemax")).toBe("318742");
    expect(thumb(w).attributes("aria-label")).toBe("Seek");
  });

  it("follows live engine updates", async () => {
    const w = await boot();
    tauri.emit("player-state", makeState({ status: "paused", position_ms: 120_000, track: makeTrack({ duration_ms: 318_742 }) }));
    await settle();
    expect(w.get('[data-testid="elapsed"]').text()).toBe("2:00");
    expect(thumb(w).attributes("aria-valuenow")).toBe("120000");
  });

  it("is disabled, with placeholder duration, when the duration is unknown", async () => {
    const w = await boot(makeState({ status: "stopped", track: makeTrack({ duration_ms: null as never }) }));
    expect(w.get('[data-testid="duration"]').text()).toBe("--:--");
    expect(thumb(w).attributes("data-disabled")).toBeDefined();
  });

  it("can be disabled by the parent (nothing playing)", async () => {
    tauri.on("get_state", makeState());
    const { wrapper } = mountApp(SeekBar, { disabled: true });
    await usePlayerStore().init();
    await settle();
    expect(wrapper.find('[role="slider"]').attributes("data-disabled")).toBeDefined();
  });

  it("seeks by 1 s with the arrow keys, sending one seek per step", async () => {
    const w = await boot();
    await thumb(w).trigger("keydown", { key: "ArrowRight" });
    await settle();
    expect(tauri.callsTo("seek_ms")).toEqual([{ ms: 84_000 }]);
    // The optimistic playhead moved with it.
    expect(w.get('[data-testid="elapsed"]').text()).toBe("1:24");
  });

  it("jumps to the start/end with Home/End", async () => {
    const w = await boot();
    await thumb(w).trigger("keydown", { key: "Home" });
    await settle();
    await thumb(w).trigger("keydown", { key: "End" });
    await settle();
    expect(tauri.callsTo("seek_ms").map((c) => c?.ms)).toEqual([0, 318_742]);
  });

  it("shows the scrub position (not the engine's) while dragging, and seeks once on release", async () => {
    const w = await boot();
    // Drive the component's public contract: live update then commit.
    const slider = w.findComponent({ name: "UiSlider" });
    slider.vm.$emit("update:modelValue", 200_000);
    await settle();
    expect(w.get('[data-testid="elapsed"]').text()).toBe("3:20");
    expect(tauri.callsTo("seek_ms")).toHaveLength(0); // dragging alone does not seek

    // An engine event arriving mid-drag must not yank the label back.
    tauri.emit("player-state", makeState({ status: "playing", position_ms: 84_000, track: makeTrack({ duration_ms: 318_742 }) }));
    await settle();
    expect(w.get('[data-testid="elapsed"]').text()).toBe("3:20");

    slider.vm.$emit("commit", 200_000);
    await settle();
    expect(tauri.callsTo("seek_ms")).toEqual([{ ms: 200_000 }]);
  });

  it("uses the larger label style at size=md", async () => {
    tauri.on("get_state", makeState());
    const { wrapper } = mountApp(SeekBar, { size: "md" });
    await usePlayerStore().init();
    await settle();
    expect(wrapper.get('[data-testid="elapsed"]').classes().join(" ")).toContain("text-xs");
  });
});

describe("SeekBar buffered fill", () => {
  it("draws data received from the server behind the playhead, and nothing when unknown", async () => {
    tauri.on("get_state", makeState({ status: "playing", position_ms: 10_000, buffered_ms: 60_000, track: makeTrack({ duration_ms: 240_000 }) }));
    const { wrapper } = mountApp(SeekBar);
    await usePlayerStore().init();
    await settle();
    expect(wrapper.get('[data-testid="buffered"]').attributes("style")).toContain("width: 25%");
    tauri.emit("player-state", makeState({ status: "playing", position_ms: 10_000, buffered_ms: null, track: makeTrack({ duration_ms: 240_000 }) }));
    await settle();
    expect(wrapper.find('[data-testid="buffered"]').exists()).toBe(false);
  });
});
