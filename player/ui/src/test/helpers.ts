import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import type { Component } from "vue";

/** Fresh pinia + attach to the document so Reka portals and focus work. */
export function mountApp(
  component: Component,
  props: Record<string, unknown> = {},
  slots: Record<string, string> = {},
  /** Runs after pinia is active but before the component mounts, to seed stores. */
  prepare?: () => void,
) {
  const pinia = createPinia();
  setActivePinia(pinia);
  prepare?.();
  const wrapper = mount(component, { props: props as never, slots, attachTo: document.body, global: { plugins: [pinia] } });
  return { wrapper, pinia };
}

export const settle = async (): Promise<void> => {
  await flushPromises();
  await flushPromises();
};

/** Elements Reka portals into <body>. */
export const $$ = (selector: string): HTMLElement[] => Array.from(document.body.querySelectorAll<HTMLElement>(selector));
export const options = (): HTMLElement[] => $$('[role="option"]');
export const menuItems = (): HTMLElement[] => $$('[role="menuitem"]');
export const dialog = (): HTMLElement | null => document.body.querySelector('[role="dialog"]');

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

/** Open a Reka dropdown menu from its trigger (Enter, as a keyboard user would). */
export async function openMenu(trigger: HTMLElement): Promise<void> {
  trigger.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settle();
}

export function key(el: Element, k: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init });
  el.dispatchEvent(e);
  return e;
}

export const byText = (root: ParentNode, selector: string, text: string | RegExp): HTMLElement | undefined =>
  Array.from(root.querySelectorAll<HTMLElement>(selector)).find((e) =>
    typeof text === "string" ? e.textContent?.includes(text) : text.test(e.textContent ?? ""),
  );

export type Wrapper = VueWrapper<never>;

/** Type into an input and fire the `input` event v-model listens to. */
export async function typeInto(input: HTMLInputElement, text: string): Promise<void> {
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

/** JSON body of a recorded fetch call. */
export const bodyOf = (init?: RequestInit): unknown => (init?.body ? JSON.parse(String(init.body)) : undefined);
