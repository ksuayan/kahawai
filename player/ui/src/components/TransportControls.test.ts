import { describe, expect, it } from "vitest";
import { makeState } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { usePlayerStore } from "../stores/player";
import TransportControls from "./TransportControls.vue";

async function boot(state = makeState({ status: "playing" }), props: Record<string, unknown> = {}) {
  tauri.on("get_state", state);
  const { wrapper } = mountApp(TransportControls, props);
  await usePlayerStore().init();
  await settle();
  return wrapper;
}
const btn = (w: Awaited<ReturnType<typeof boot>>, label: string | RegExp) =>
  w.findAll("button").find((b) => (typeof label === "string" ? b.attributes("aria-label") === label : label.test(b.attributes("aria-label") ?? "")))!;

describe("TransportControls", () => {
  it("shows Pause while playing and Play otherwise, and toggles on click", async () => {
    const playing = await boot(makeState({ status: "playing" }));
    expect(btn(playing, /Pause|Play/).attributes("aria-label")).toBe("Pause");
    await btn(playing, "Pause").trigger("click");
    expect(tauri.callsTo("toggle")).toHaveLength(1);

    const paused = await boot(makeState({ status: "paused" }));
    expect(paused.findAll('[data-testid="play-pause"]').at(-1)!.attributes("aria-label")).toBe("Play");
  });

  it("shows a loading marker while the stream opens", async () => {
    const w = await boot(makeState({ status: "loading" }));
    expect(w.findAll('[data-testid="play-pause"]').at(-1)!.text()).toBe("…");
  });

  it("wires next, previous, stop, shuffle and repeat to the bridge", async () => {
    const w = await boot();
    await btn(w, "Next").trigger("click");
    await btn(w, "Previous").trigger("click");
    await btn(w, "Stop").trigger("click");
    await btn(w, "Shuffle").trigger("click");
    await btn(w, /^Repeat/).trigger("click");
    await settle();
    const cmds = tauri.calls.map((c) => c.cmd);
    expect(cmds).toEqual(expect.arrayContaining(["next_track", "prev_track", "stop", "set_shuffle", "set_repeat"]));
  });

  it("reflects shuffle and repeat state as pressed toggles", async () => {
    const w = await boot(makeState({ status: "playing", shuffle: true, repeat: "all" }));
    expect(btn(w, "Shuffle").attributes("aria-pressed")).toBe("true");
    expect(btn(w, "Repeat all").attributes("aria-pressed")).toBe("true");
    const off = await boot(makeState({ status: "playing", shuffle: false, repeat: "off" }));
    expect(btn(off, "Shuffle").attributes("aria-pressed")).toBeUndefined();
    expect(btn(off, "Repeat off").attributes("aria-pressed")).toBeUndefined();
  });

  it("labels repeat-one distinctly", async () => {
    const w = await boot(makeState({ repeat: "one" }));
    expect(btn(w, "Repeat one").text()).toBe("🔂");
  });

  it("can hide Stop (full-page view) and disable everything (nothing playing)", async () => {
    const noStop = await boot(makeState(), { showStop: false });
    expect(noStop.findAll("button").some((b) => b.attributes("aria-label") === "Stop")).toBe(false);
    const off = await boot(makeState(), { disabled: true });
    expect(off.findAll("button").every((b) => b.attributes("disabled") !== undefined)).toBe(true);
  });
});
