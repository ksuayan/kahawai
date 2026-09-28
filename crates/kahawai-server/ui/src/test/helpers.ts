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
