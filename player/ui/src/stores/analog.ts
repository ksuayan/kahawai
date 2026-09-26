import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { getDspSettings, setAnalog } from "../tauri";
import { clampAnalog, DEFAULT_ANALOG_SETTINGS, FLAVOUR_INFO, type AnalogFlavour, type AnalogSettings, type ListeningRecipe } from "../types";

export type Slot = "a" | "b";
const KEY = "kahawai-player.analog-ab";

interface Saved {
  a: AnalogSettings;
  b: AnalogSettings;
  active: Slot;
}

function load(): Saved | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as Partial<Saved>;
    if (!v.a || !v.b) return null;
    return { a: clampAnalog({ ...DEFAULT_ANALOG_SETTINGS, ...v.a }), b: clampAnalog({ ...DEFAULT_ANALOG_SETTINGS, ...v.b }), active: v.active === "b" ? "b" : "a" };
  } catch {
    return null;
  }
}

/**
 * A/B comparison of analog-warmth settings. Two slots each hold a full set
 * of settings; the active slot is what the engine plays. Switching slots
 * swaps the settings in the engine (it fades, so there is no click), so you
 * can compare flavours, drive, or anti-aliasing plans while music plays.
 * Slot A starts as "off" (the dry signal) and slot B as the warm triode.
 * The engine remembers only the active slot; the pair itself is kept here.
 */
export const useAnalogStore = defineStore("analog", () => {
  const a = ref<AnalogSettings>({ ...DEFAULT_ANALOG_SETTINGS, enabled: false });
  const b = ref<AnalogSettings>({ ...DEFAULT_ANALOG_SETTINGS, enabled: true });
  const active = ref<Slot>("a");
  const loaded = ref(false);

  const slots = { a, b };
  const current = computed(() => slots[active.value].value);

  function persist(): void {
    try {
      localStorage.setItem(KEY, JSON.stringify({ a: a.value, b: b.value, active: active.value }));
    } catch {
      /* storage unavailable: the pair lasts for this session */
    }
  }

  function push(): void {
    void setAnalog(current.value);
    persist();
  }

  /** On boot: restore the pair, or seed it from what the engine has saved. */
  async function init(): Promise<void> {
    const saved = load();
    if (saved) {
      a.value = saved.a;
      b.value = saved.b;
      active.value = saved.active;
    } else {
      const engine = (await getDspSettings())?.analog;
      if (engine?.enabled) {
        b.value = clampAnalog({ ...DEFAULT_ANALOG_SETTINGS, ...engine });
        active.value = "b";
      }
    }
    loaded.value = true;
  }

  /** Change one slot. Only the active slot reaches the engine. */
  function update(slot: Slot, patch: Partial<AnalogSettings>): void {
    slots[slot].value = clampAnalog({ ...slots[slot].value, ...patch });
    if (slot === active.value) push();
    else persist();
  }

  /** Choose a flavour for a slot; Sag and Transformer take that flavour's typical values (adjust them afterwards). */
  function setFlavour(slot: Slot, flavour: AnalogFlavour): void {
    const info = FLAVOUR_INFO[flavour];
    if (!info) return;
    update(slot, { flavour, sag: info.sag, transformer: info.transformer });
  }

  /** The full settings a recipe means for one slot: defaults, then the flavour's typical sag/transformer, then the recipe's own values. */
  function recipeSlot(part: Partial<AnalogSettings>): AnalogSettings {
    const flavour = part.flavour ?? DEFAULT_ANALOG_SETTINGS.flavour;
    const info = FLAVOUR_INFO[flavour];
    return clampAnalog({ ...DEFAULT_ANALOG_SETTINGS, sag: info.sag, transformer: info.transformer, ...part });
  }

  /** Load a ready-made comparison into A and B and start on A. */
  function applyRecipe(recipe: ListeningRecipe): void {
    a.value = recipeSlot(recipe.a);
    b.value = recipeSlot(recipe.b);
    active.value = "a";
    push();
  }

  /** Listen to a slot: its settings become the engine's. */
  function select(slot: Slot): void {
    if (active.value === slot) return;
    active.value = slot;
    push();
  }

  function toggle(): void {
    select(active.value === "a" ? "b" : "a");
  }

  /** Copy one slot's settings over the other (handy for tweaking one setting). */
  function copy(from: Slot, to: Slot): void {
    if (from === to) return;
    update(to, { ...slots[from].value });
  }

  return { a, b, active, current, loaded, init, update, setFlavour, applyRecipe, select, toggle, copy };
});
