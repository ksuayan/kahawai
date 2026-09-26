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

  it("uses real icons, not emoji or glyph characters, and stays accessible through labels", async () => {
    const w = await boot(makeState({ status: "playing" }));
    const buttons = w.findAll("button");
    expect(buttons.length).toBe(6);
    for (const b of buttons) {
      expect(b.find("svg").exists()).toBe(true);
      expect(b.find("svg").attributes("aria-hidden")).toBe("true");
      expect(b.text()).toBe(""); // no glyph text: the name comes from aria-label
      expect(b.attributes("aria-label")).toBeTruthy();
    }
    const kinds = buttons.map((b) => b.find("svg").classes().find((c) => /^lucide-[a-z0-9-]+$/.test(c) && !c.endsWith("-icon")));
    expect(new Set(kinds).size).toBe(6); // six different icons
  });

  it("shows a loading marker while the stream opens", async () => {
    const w = await boot(makeState({ status: "loading" }));
    const btn = w.findAll('[data-testid="play-pause"]').at(-1)!;
    expect(btn.find("svg.animate-spin").exists()).toBe(true);
    expect(btn.text()).toBe("");
  });

  it("wires next, previous, shuffle and repeat to the bridge", async () => {
    const w = await boot();
    await btn(w, "Next").trigger("click");
    await btn(w, "Previous").trigger("click");
    await btn(w, "Shuffle").trigger("click");
    await btn(w, /^Repeat/).trigger("click");
    await settle();
    const cmds = tauri.calls.map((c) => c.cmd);
    expect(cmds).toEqual(expect.arrayContaining(["next_track", "prev_track", "set_shuffle", "set_repeat"]));
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
    const icon = btn(w, "Repeat one").find("svg");
    expect(icon.classes().join(" ")).toMatch(/lucide-repeat-?1/);
    const all = await boot(makeState({ repeat: "all" }));
    expect(btn(all, "Repeat all").find("svg").classes().join(" ")).not.toMatch(/lucide-repeat-?1/);
  });

  it("has no Stop button, groups shuffle/repeat apart from the main buttons, and disables all when idle", async () => {
    const w = await boot(makeState());
    expect(w.findAll("button").some((b) => b.attributes("aria-label") === "Stop")).toBe(false);
    const main = w.get('[data-testid="main-controls"]');
    const mode = w.get('[data-testid="mode-controls"]');
    expect(main.findAll("button").map((b) => b.attributes("aria-label"))).toEqual(["Previous", expect.stringMatching(/^(Play|Pause)$/), "Next"]);
    expect(mode.findAll("button").map((b) => b.attributes("aria-label"))).toEqual(["Shuffle", expect.stringMatching(/^Repeat/), "Equalizer"]);
    const off = await boot(makeState(), { disabled: true });
    expect(off.findAll("button").every((b) => b.attributes("disabled") !== undefined)).toBe(true);
  });
});
