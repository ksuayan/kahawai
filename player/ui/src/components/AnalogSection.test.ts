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
    expect(wrapper.text()).toContain("Analog warmth (experimental)");
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
