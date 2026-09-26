import { beforeEach, describe, expect, it } from "vitest";
import { makeState } from "../test/fixtures";
import { $$, mountApp, openSelect, options, pick, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useAnalogStore } from "../stores/analog";
import { usePlayerStore } from "../stores/player";
import { ANALOG_FLAVOURS, FLAVOUR_INFO, LISTENING_RECIPES } from "../types";
import AnalogSection from "./AnalogSection.vue";

async function boot(state = makeState({ status: "playing", output_path: "pcm-shared" })) {
  tauri.on("get_state", state);
  const { wrapper } = mountApp(AnalogSection);
  await usePlayerStore().init();
  await settle();
  return { wrapper, store: useAnalogStore() };
}
const sent = () => tauri.callsTo("set_analog").map((c) => (c as { settings: Record<string, unknown> }).settings);
const slider = (label: string) => document.body.querySelector(`[role="slider"][aria-label="${label}"]`) as HTMLElement;

beforeEach(() => localStorage.clear());

describe("Analog warmth settings", () => {
  it("shows two slots, A dry and B warm, with A being heard", async () => {
    const { wrapper } = await boot();
    expect(wrapper.text()).toMatch(/analog warmth \(experimental\)/i);
    expect(wrapper.get('[data-testid="ab-a"]').attributes("aria-pressed")).toBe("true");
    expect(wrapper.get('[data-testid="ab-b"]').attributes("aria-pressed")).toBe("false");
    expect(wrapper.get('[data-testid="slot-a-summary"]').text()).toBe("Off (dry signal)");
    expect(wrapper.get('[data-testid="slot-b-summary"]').text()).toContain("12AX7 · drive 40% · mix 40%");
  });

  it("A/B buttons and the switch button change what the engine plays", async () => {
    const { wrapper } = await boot();
    await wrapper.get('[data-testid="ab-b"]').trigger("click");
    expect(sent().at(-1)).toMatchObject({ enabled: true, flavour: "warm_triode" });
    expect(wrapper.get('[data-testid="ab-b"]').attributes("aria-pressed")).toBe("true");
    await wrapper.get('[data-testid="ab-toggle"]').trigger("click");
    expect(sent().at(-1)).toMatchObject({ enabled: false });
    expect(wrapper.get('[data-testid="ab-a"]').attributes("aria-pressed")).toBe("true");
  });

  it("sliders edit the slot and go live only for the slot being heard", async () => {
    const { wrapper, store } = await boot();
    await wrapper.get('[data-testid="ab-b"]').trigger("click");
    tauri.calls.length = 0;
    slider("Drive B").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    await settle();
    expect(store.b.drive).toBeCloseTo(0.41, 5);
    expect(sent().at(-1)).toMatchObject({ drive: 0.41 });
    const before = sent().length;
    slider("Mix A").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    await settle();
    expect(store.a.mix).toBeCloseTo(0.41, 5);
    expect(sent()).toHaveLength(before); // A is not being heard
  });

  it("offers both flavours and every anti-aliasing plan", async () => {
    await boot();
    await openSelect(document.body.querySelector('[aria-label="Flavour B"]') as HTMLElement);
    expect(options()).toHaveLength(21);
    expect(options().map((o) => o.textContent?.trim())).toEqual(ANALOG_FLAVOURS.map((f) => FLAVOUR_INFO[f].label));
    pick(options()[8]); // 300B
    await settle();
    expect(useAnalogStore().b).toMatchObject({ flavour: "tube_300b", sag: 0.4, transformer: 0.5 });
    await openSelect(document.body.querySelector('[aria-label="Anti-aliasing B"]') as HTMLElement);
    expect(options().map((o) => o.textContent?.trim())).toEqual([
      "Auto (by sample rate)", "1x, no protection", "1x + ADAA", "2x oversampling", "2x oversampling + ADAA", "4x oversampling", "4x oversampling + ADAA",
    ]);
    pick(options()[6]);
    await settle();
    expect(useAnalogStore().b.antialias).toBe("x4_adaa");
  });

  it("copies a slot to the other", async () => {
    const { wrapper, store } = await boot();
    store.update("b", { flavour: "solid_state", drive: 0.9 });
    await settle();
    await wrapper.get('[data-testid="copy-b"]').trigger("click");
    expect(store.a).toMatchObject({ flavour: "solid_state", drive: 0.9 });
  });

  it("reports what the stage is doing, from the engine", async () => {
    const { wrapper } = await boot(makeState({ status: "playing", output_path: "pcm-shared", analog_plan: "4x oversampling + ADAA, 0.7 ms latency" }));
    expect(wrapper.get('[data-testid="analog-status"]').text()).toContain("4x oversampling + ADAA, 0.7 ms latency");
    const idle = await boot();
    expect(idle.wrapper.get('[data-testid="analog-status"]').text()).toContain("off, or nothing is playing");
  });

  it.each(["dop-exclusive", "pcm-exclusive"] as const)("is dimmed with an explanation on %s", async (path) => {
    const { wrapper } = await boot(makeState({ status: "playing", output_path: path }));
    expect(wrapper.get('[data-testid="analog-unsupported"]').text()).toContain("not supported for this stream type");
    const editor = wrapper.get('[data-testid="analog-editor"]');
    expect(editor.classes().join(" ")).toContain("opacity-40");
    expect(editor.attributes("inert")).toBeDefined();
  });

  it("has labelled controls for both slots", async () => {
    await boot();
    for (const s of ["A", "B"]) {
      for (const name of ["Drive", "Mix", "Sag", "Transformer", "Output"]) expect(slider(`${name} ${s}`)).not.toBeNull();
      expect($$(`[aria-label="Flavour ${s}"]`)).toHaveLength(1);
    }
  });
});

describe("Analog warmth: sag and transformer", () => {
  it("edits them per slot and sends them to the engine when that slot is heard", async () => {
    const { wrapper, store } = await boot();
    await wrapper.get('[data-testid="ab-b"]').trigger("click");
    tauri.calls.length = 0;
    document.body.querySelector('[role="slider"][aria-label="Sag B"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    document.body.querySelector('[role="slider"][aria-label="Transformer B"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    await settle();
    expect(store.b.sag).toBeCloseTo(0.31, 5);
    expect(store.b.transformer).toBeCloseTo(0.29, 5);
    expect(tauri.callsTo("set_analog").at(-1)).toMatchObject({ settings: { sag: 0.31, transformer: 0.29 } });
  });
});

describe("Analog warmth: listening suggestions", () => {
  it("lists every recipe with what to play and listen for, and loads one into A and B", async () => {
    const { wrapper, store } = await boot();
    const recipes = wrapper.get('[data-testid="recipes"]');
    expect(recipes.text()).toContain("Listening suggestions");
    expect(recipes.findAll("details")).toHaveLength(LISTENING_RECIPES.length);
    expect(recipes.text()).toContain("Play:");
    expect(recipes.text()).toContain("Listen for:");
    await wrapper.get('[data-testid="use-el34-vs-6l6gc"]').trigger("click");
    expect(store.a.flavour).toBe("push_pull_el34");
    expect(store.b.flavour).toBe("push_pull_6l6gc");
    expect(wrapper.get('[data-testid="recipe-applied"]').text()).toContain("British against American power stages");
    expect(wrapper.get('[data-testid="slot-a-summary"]').text()).toContain("EL34 pair");
    expect(tauri.callsTo("set_analog").at(-1)).toMatchObject({ settings: { flavour: "push_pull_el34" } });
  });
});

describe("Analog warmth: level meter", () => {
  const lvl = (delta_db: number, peak_dbfs = -6, seconds = 5) => ({ input_lufs: -20, output_lufs: -20 + delta_db, delta_db, peak_dbfs, seconds });
  const heardB = async (state: ReturnType<typeof makeState>) => {
    const b = await boot(state);
    await b.wrapper.get('[data-testid="ab-b"]').trigger("click");
    await settle();
    return b;
  };

  it("shows before, after, change and peak for the slot being heard, with a marker on the scale", async () => {
    const { wrapper } = await heardB(makeState({ status: "playing", output_path: "pcm-shared", analog_level: lvl(1.8, -4.2) }));
    expect(wrapper.get('[data-testid="meter-in"]').text()).toBe("-20.0");
    expect(wrapper.get('[data-testid="meter-out"]').text()).toBe("-18.2");
    expect(wrapper.get('[data-testid="meter-delta"]').text()).toBe("+1.8 dB");
    expect(wrapper.get('[data-testid="meter-peak"]').text()).toBe("-4.2");
    expect(wrapper.get('[data-testid="meter-marker"]').attributes("style")).toContain("calc(65%"); // (1.8 + 6) / 12
  });

  it("colours the change by how far it is from level: ok, warning, bad", async () => {
    for (const [delta, state] of [[0.2, "ok"], [1.0, "warn"], [-3, "bad"]] as const) {
      const { wrapper } = await heardB(makeState({ status: "playing", output_path: "pcm-shared", analog_level: lvl(delta) }));
      expect(wrapper.get('[data-testid="meter-delta"]').attributes("data-state")).toBe(state);
      wrapper.unmount();
    }
  });

  it("warns when the peak is at full scale", async () => {
    const quiet = await heardB(makeState({ status: "playing", output_path: "pcm-shared", analog_level: lvl(0, -3) }));
    expect(quiet.wrapper.find('[data-testid="meter-clip"]').exists()).toBe(false);
    quiet.wrapper.unmount();
    const hot = await heardB(makeState({ status: "playing", output_path: "pcm-shared", analog_level: lvl(0, 0.4) }));
    expect(hot.wrapper.get('[data-testid="meter-clip"]').text()).toContain("may clip");
  });

  it("says why there is no reading: dry slot, nothing playing, or still measuring", async () => {
    const dry = await boot(makeState({ status: "playing", output_path: "pcm-shared" }));
    expect(dry.wrapper.get('[data-testid="meter-idle"]').text()).toContain("dry signal");
    dry.wrapper.unmount();
    const idle = await heardB(makeState({ status: "stopped", output_path: "pcm-shared" }));
    expect(idle.wrapper.get('[data-testid="meter-idle"]').text()).toContain("Play music");
    idle.wrapper.unmount();
    const measuring = await heardB(makeState({ status: "playing", output_path: "pcm-shared" }));
    expect(measuring.wrapper.get('[data-testid="meter-idle"]').text()).toContain("Measuring");
  });

  it("match buttons need both slots measured, then trim the Output and explain what changed", async () => {
    const { wrapper, store } = await boot();
    expect(wrapper.get('[data-testid="match-b"]').attributes("disabled")).toBeDefined();
    expect(wrapper.get('[data-testid="measured-a"]').text()).toContain("0.0 dB (dry)");
    expect(wrapper.get('[data-testid="measured-b"]').text()).toContain("not measured yet");
    store.measured.b = 2.4;
    await settle();
    expect(wrapper.get('[data-testid="match-b"]').attributes("disabled")).toBeUndefined();
    await wrapper.get('[data-testid="match-b"]').trigger("click");
    await settle();
    expect(store.b.output_db).toBe(-2.5);
    expect(wrapper.get('[data-testid="match-note"]').text()).toContain("Output of B to -2.5 dB");
    expect(wrapper.get('[role="slider"][aria-label="Output B"]').attributes("aria-valuenow")).toBe("-2.5");
  });
});

describe("Analog warmth: blind test", () => {
  const ready = async () => {
    const b = await boot(makeState({ status: "playing", output_path: "pcm-shared" }));
    b.store.update("b", { flavour: "tube_300b" });
    b.store.measured.b = 0.1;
    await settle();
    return b;
  };
  const startBlind = async (wrapper: Awaited<ReturnType<typeof boot>>["wrapper"]) => {
    await wrapper.get('[data-testid="blind-start-button"]').trigger("click");
    await settle();
  };

  it("can only start when the two slots differ and are level, unless you insist", async () => {
    const { wrapper, store } = await boot();
    // Fresh: B unmeasured.
    expect(wrapper.get('[data-testid="blind-start-button"]').attributes("disabled")).toBeDefined();
    expect(wrapper.get('[data-testid="blind-check"]').text()).toContain("Measure both slots");
    store.measured.b = 2.0; // measured but 2 dB louder
    await settle();
    expect(wrapper.get('[data-testid="blind-check"]').text()).toContain("levels differ by 2.0 dB");
    expect(wrapper.get('[data-testid="blind-start-button"]').attributes("disabled")).toBeDefined();
    await wrapper.get('[data-testid="blind-anyway"]').setValue(true);
    expect(wrapper.get('[data-testid="blind-start-button"]').attributes("disabled")).toBeUndefined();
    store.measured.b = 0.1;
    await settle();
    expect(wrapper.get('[data-testid="blind-check"]').text()).toContain("Levels are matched");
  });

  it("hides everything that would give X away while it runs", async () => {
    const { wrapper } = await ready();
    expect(wrapper.find('[data-testid="slots"]').exists()).toBe(true);
    await startBlind(wrapper);
    expect(wrapper.get('[data-testid="blind-test"]').text()).toContain("Trial 1 of 10");
    for (const hidden of ["slots", "level-meter", "recipes", "analog-status", "ab-a", "ab-b", "ab-toggle"]) {
      expect(wrapper.find(`[data-testid="${hidden}"]`).exists(), hidden).toBe(false);
    }
    // The panel names no flavour and shows no settings (the intro text above lists tubes generically).
    const panel = wrapper.get('[data-testid="blind-test"]').text();
    expect(panel).not.toMatch(/300B|JFET|12AX7|drive|mix|dB/i);
  });

  it("lets you hear A, B and X, and answer; then shows the score and the reveal", async () => {
    const { wrapper, store } = await ready();
    await openSelect(document.body.querySelector('[aria-label="Number of trials"]') as HTMLElement);
    pick(options()[0]); // 5
    await settle();
    await startBlind(wrapper);
    expect(wrapper.get('[data-testid="blind-progress"]').text()).toContain("of 5");
    await wrapper.get('[data-testid="blind-hear-x"]').trigger("click");
    expect(wrapper.get('[data-testid="blind-hear-x"]').attributes("aria-pressed")).toBe("true");
    expect(wrapper.get('[data-testid="blind-heard"]').text()).toBe("Hearing X");
    expect(store.a.enabled).toBe(false); // untouched: the test never edits the slots
    for (let i = 0; i < 5; i++) {
      await wrapper.get('[data-testid="blind-answer-a"]').trigger("click");
    }
    await settle();
    expect(wrapper.find('[data-testid="blind-test"]').exists()).toBe(false);
    const result = wrapper.get('[data-testid="blind-result"]');
    expect(result.get('[data-testid="blind-verdict"]').text()).toContain("of 5 right");
    expect(result.findAll("li")).toHaveLength(5);
    expect(result.text()).toMatch(/X was [AB], you said A/);
    await wrapper.get('[data-testid="blind-dismiss"]').trigger("click");
    expect(wrapper.find('[data-testid="blind-result"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="slots"]').exists()).toBe(true); // everything is back
  });

  it("can be cancelled", async () => {
    const { wrapper } = await ready();
    await startBlind(wrapper);
    await wrapper.get('[data-testid="blind-cancel"]').trigger("click");
    expect(wrapper.find('[data-testid="blind-test"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="slots"]').exists()).toBe(true);
  });
});
