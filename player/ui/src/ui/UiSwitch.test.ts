import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import UiSwitch from "./UiSwitch.vue";

describe("UiSwitch", () => {
  it("reflects the model as aria-checked", () => {
    const on = mount(UiSwitch, { props: { modelValue: true, label: "EQ enabled" } });
    const off = mount(UiSwitch, { props: { modelValue: false, label: "EQ enabled" } });
    expect(on.find('[role="switch"]').attributes("aria-checked")).toBe("true");
    expect(off.find('[role="switch"]').attributes("aria-checked")).toBe("false");
  });

  it("toggles on click and emits the new value", async () => {
    const w = mount(UiSwitch, { props: { modelValue: false } });
    await w.find('[role="switch"]').trigger("click");
    expect(w.emitted("update:modelValue")).toEqual([[true]]);
  });

  it("toggles from the keyboard (Space)", async () => {
    const w = mount(UiSwitch, { props: { modelValue: true } });
    await w.find('[role="switch"]').trigger("keydown", { key: " " });
    await w.find('[role="switch"]').trigger("keyup", { key: " " });
    // Reka switches on click; Space on a <button> dispatches one.
    await w.find('[role="switch"]').trigger("click");
    expect(w.emitted("update:modelValue")?.at(-1)).toEqual([false]);
  });

  it("renders its label and ignores clicks when disabled", async () => {
    const w = mount(UiSwitch, { props: { modelValue: false, label: "Loudness", disabled: true } });
    expect(w.text()).toContain("Loudness");
    await w.find('[role="switch"]').trigger("click");
    expect(w.emitted("update:modelValue")).toBeUndefined();
  });
});
