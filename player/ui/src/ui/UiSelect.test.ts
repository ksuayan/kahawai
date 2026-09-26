import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, ref } from "vue";
import { describe, expect, it } from "vitest";
import UiSelect, { type UiSelectOption } from "./UiSelect.vue";

const options: UiSelectOption[] = [
  { value: null, label: "System default" },
  { value: "Built-in Output", label: "Built-in Output" },
  { value: "FIIO K15 ", label: "FIIO K15 " }, // trailing space is part of the name
  { value: "RØDE Connect", label: "RØDE Connect" },
];

function host(initial: string | null) {
  const Host = defineComponent({
    components: { UiSelect },
    setup() {
      const v = ref<string | null>(initial);
      return { v, options };
    },
    template: `<UiSelect v-model="v" :options="options" aria-label="Output device" />`,
  });
  return mount(Host, { attachTo: document.body });
}
const value = (w: ReturnType<typeof host>) => (w.vm as unknown as { v: string | null }).v;

async function open(w: ReturnType<typeof host>) {
  await w.find('[role="combobox"]').trigger("keydown", { key: "Enter" });
  await flushPromises();
}
const optionEls = () => Array.from(document.body.querySelectorAll('[role="option"]')) as HTMLElement[];

/** A real mouse selection: down, move, up (Reka selects on pointerup for mice). */
function pick(el: HTMLElement) {
  for (const type of ["pointerdown", "pointermove", "pointerup"]) {
    el.dispatchEvent(new PointerEvent(type, { pointerType: "mouse", bubbles: true }));
  }
}

describe("UiSelect", () => {
  it("shows the selected option's label, including for the null option", async () => {
    const w = host(null);
    await flushPromises();
    expect(w.find('[role="combobox"]').text()).toContain("System default");
    const w2 = host("Built-in Output");
    await flushPromises();
    expect(w2.findAll('[role="combobox"]').at(-1)!.text()).toContain("Built-in Output");
  });

  it("is labelled for assistive tech", () => {
    const w = host(null);
    expect(w.find('[role="combobox"]').attributes("aria-label")).toBe("Output device");
  });

  it("lists every option when opened", async () => {
    const w = host(null);
    await open(w);
    expect(optionEls().map((o) => o.textContent?.trim())).toEqual([
      "System default",
      "Built-in Output",
      "FIIO K15",
      "RØDE Connect",
    ]);
  });

  it("selecting an option updates v-model with the exact value", async () => {
    const w = host(null);
    await open(w);
    pick(optionEls()[3]);
    await flushPromises();
    expect(value(w)).toBe("RØDE Connect");
  });

  it("preserves trailing whitespace in values (device names)", async () => {
    const w = host(null);
    await open(w);
    pick(optionEls()[2]);
    await flushPromises();
    expect(value(w)).toBe("FIIO K15 ");
    expect(value(w)).not.toBe("FIIO K15");
  });

  it("maps the null option back to null (not a sentinel string)", async () => {
    const w = host("Built-in Output");
    await open(w);
    pick(optionEls()[0]);
    await flushPromises();
    expect(value(w)).toBeNull();
  });

  it("selects with the keyboard (focus an option, Enter) and closes", async () => {
    const w = host(null);
    await open(w);
    const o = optionEls()[1];
    o.focus();
    o.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await flushPromises();
    expect(value(w)).toBe("Built-in Output");
    expect(optionEls()).toHaveLength(0);
  });

  it("does not open when disabled", async () => {
    const w = mount(UiSelect, { props: { modelValue: null, options, disabled: true }, attachTo: document.body });
    await w.find('[role="combobox"]').trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(optionEls()).toHaveLength(0);
  });
});
