// Local copy of the player's `mountApp`/`settle` helpers: these must use
// *this* package's own `pinia`/`vue` instances (not the player's, via the
// `@pw` alias), or `useStore()` calls inside our components see a different
// Pinia instance than the one `setActivePinia` installed here.

import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import type { Component } from "vue";

export function mountApp(
  component: Component,
  props: Record<string, unknown> = {},
  slots: Record<string, string> = {},
  prepare?: () => void,
) {
  const pinia = createPinia();
  setActivePinia(pinia);
  prepare?.();
  const wrapper = mount(component, {
    props: props as never,
    slots,
    attachTo: document.body,
    global: { plugins: [pinia] },
  });
  return { wrapper, pinia };
}

export const settle = async (): Promise<void> => {
  await flushPromises();
  await flushPromises();
};

export type Wrapper = VueWrapper<never>;

// Reka Select helpers, same as the player's (DOM-only, no vue/pinia).
export const options = (): HTMLElement[] =>
  Array.from(document.body.querySelectorAll<HTMLElement>('[role="option"]'));

/** A real mouse selection: down, move, up (Reka selects on pointerup for mice). */
export function pick(el: HTMLElement): void {
  for (const type of ["pointerdown", "pointermove", "pointerup"]) {
    el.dispatchEvent(new PointerEvent(type, { pointerType: "mouse", bubbles: true }));
  }
}

/** Open a Reka Select from its trigger (Enter on the combobox). */
export async function openSelect(trigger: HTMLElement): Promise<void> {
  trigger.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settle();
}
