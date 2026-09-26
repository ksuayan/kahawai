import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, ref } from "vue";
import { describe, expect, it } from "vitest";
import UiSlider from "./UiSlider.vue";

async function mountSlider(props: Record<string, unknown>) {
  const w = mount(UiSlider, { props: props as never, attachTo: document.body });
  await flushPromises(); // Reka registers the thumb after mount
  return w;
}
const thumb = (w: ReturnType<typeof mount>) => w.find('[role="slider"]');

describe("UiSlider", () => {
  it("renders the value on the thumb with the given range", async () => {
    const t = thumb(await mountSlider({ modelValue: 30, min: 0, max: 200 }));
    expect(t.attributes("aria-valuenow")).toBe("30");
    expect(t.attributes("aria-valuemin")).toBe("0");
    expect(t.attributes("aria-valuemax")).toBe("200");
  });

  it("clamps an out-of-range value into the range", async () => {
    const t = thumb(await mountSlider({ modelValue: 999, min: 0, max: 100 }));
    expect(t.attributes("aria-valuenow")).toBe("100");
    const low = thumb(await mountSlider({ modelValue: -5, min: 0, max: 100 }));
    expect(low.attributes("aria-valuenow")).toBe("0");
  });

  it("gives a zero-length range (unknown duration) a usable 1-wide range", async () => {
    const t = thumb(await mountSlider({ modelValue: 0, min: 0, max: 0 }));
    expect(t.attributes("aria-valuemax")).toBe("1");
  });

  it("emits live updates and a commit on keyboard steps (v-model)", async () => {
    const commits: number[] = [];
    const Host = defineComponent({
      components: { UiSlider },
      setup() {
        const v = ref(50);
        return { v, commits };
      },
      template: `<UiSlider v-model="v" :step="5" @commit="(x) => commits.push(x)" />`,
    });
    const w = mount(Host, { attachTo: document.body });
    await flushPromises();
    const t = thumb(w);
    await t.trigger("keydown", { key: "ArrowRight" });
    expect((w.vm as unknown as { v: number }).v).toBe(55);
    await t.trigger("keydown", { key: "ArrowLeft" });
    await t.trigger("keydown", { key: "ArrowLeft" });
    expect((w.vm as unknown as { v: number }).v).toBe(45);
    await t.trigger("keydown", { key: "End" });
    expect((w.vm as unknown as { v: number }).v).toBe(100);
    await t.trigger("keydown", { key: "Home" });
    expect((w.vm as unknown as { v: number }).v).toBe(0);
    expect(commits).toEqual([55, 50, 45, 100, 0]);
  });

  it("does not respond when disabled", async () => {
    const w = await mountSlider({ modelValue: 50, disabled: true });
    await thumb(w).trigger("keydown", { key: "ArrowRight" });
    expect(w.emitted("update:modelValue")).toBeUndefined();
    expect(w.emitted("commit")).toBeUndefined();
  });

  it("labels the thumb for assistive tech", async () => {
    const w = await mountSlider({ modelValue: 1, ariaLabel: "Volume" });
    expect(thumb(w).attributes("aria-label")).toBe("Volume");
  });
});
