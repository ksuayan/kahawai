import { describe, expect, it } from "vitest";
import { defineComponent, ref } from "vue";
import { mount } from "@vue/test-utils";
import ConfirmDialog from "./ConfirmDialog.vue";
import PromptDialog from "./PromptDialog.vue";
import { $$, dialog, key, settle, typeInto } from "../test/helpers";

const buttons = () => $$("[role=dialog] button");
const btn = (label: string) => buttons().find((b) => b.textContent?.trim() === label)!;

function promptHost(initial = "") {
  const submitted: string[] = [];
  const Host = defineComponent({
    components: { PromptDialog },
    setup() {
      const open = ref(false);
      return { open, submitted, initial };
    },
    template: `<PromptDialog v-model:open="open" title="New playlist" label="Playlist name" :initial="initial" confirm-label="Create" @submit="(v) => submitted.push(v)" />`,
  });
  const w = mount(Host, { attachTo: document.body });
  const vm = w.vm as unknown as { open: boolean };
  return { w, vm, submitted };
}
const input = () => document.body.querySelector<HTMLInputElement>("[role=dialog] input")!;

describe("PromptDialog (replaces window.prompt)", () => {
  it("is hidden until opened, then shows the title, label and an input", async () => {
    const { vm } = promptHost();
    expect(dialog()).toBeNull();
    vm.open = true;
    await settle();
    expect(dialog()).not.toBeNull();
    expect(dialog()!.textContent).toContain("New playlist");
    expect(dialog()!.textContent).toContain("Playlist name");
    expect(input()).not.toBeNull();
  });

  it("disables the confirm button until something non-blank is typed", async () => {
    const { vm } = promptHost();
    vm.open = true;
    await settle();
    expect(btn("Create").hasAttribute("disabled")).toBe(true);
    await typeInto(input(), "   ");
    expect(btn("Create").hasAttribute("disabled")).toBe(true);
    await typeInto(input(), "Road trip");
    expect(btn("Create").hasAttribute("disabled")).toBe(false);
  });

  it("submits the trimmed name and closes", async () => {
    const { vm, submitted } = promptHost();
    vm.open = true;
    await settle();
    await typeInto(input(), "  Road trip  ");
    btn("Create").click();
    await settle();
    expect(submitted).toEqual(["Road trip"]);
    expect(vm.open).toBe(false);
    expect(dialog()).toBeNull();
  });

  it("submits on Enter, and never submits an empty name", async () => {
    const { vm, submitted } = promptHost();
    vm.open = true;
    await settle();
    key(input(), "Enter");
    await settle();
    expect(submitted).toEqual([]);
    expect(vm.open).toBe(true);

    await typeInto(input(), "Focus");
    key(input(), "Enter");
    await settle();
    expect(submitted).toEqual(["Focus"]);
    expect(vm.open).toBe(false);
  });

  it("Cancel closes without submitting", async () => {
    const { vm, submitted } = promptHost();
    vm.open = true;
    await settle();
    await typeInto(input(), "Nope");
    btn("Cancel").click();
    await settle();
    expect(submitted).toEqual([]);
    expect(vm.open).toBe(false);
  });

  it("Escape closes the dialog", async () => {
    const { vm } = promptHost();
    vm.open = true;
    await settle();
    key(dialog()!, "Escape");
    await settle();
    expect(vm.open).toBe(false);
  });

  it("starts each opening from the initial text (no leftover from last time)", async () => {
    const { vm } = promptHost("My list");
    vm.open = true;
    await settle();
    expect(input().value).toBe("My list");
    await typeInto(input(), "Changed");
    btn("Cancel").click();
    await settle();
    vm.open = true;
    await settle();
    expect(input().value).toBe("My list");
  });

  it("labels the input for assistive tech", async () => {
    const { vm } = promptHost();
    vm.open = true;
    await settle();
    expect(input().getAttribute("aria-label")).toBe("Playlist name");
  });
});

describe("ConfirmDialog (replaces window.confirm)", () => {
  function confirmHost(danger = false) {
    let confirmed = 0;
    const Host = defineComponent({
      components: { ConfirmDialog },
      setup() {
        const open = ref(false);
        return { open, danger, onConfirm: () => confirmed++ };
      },
      template: `<ConfirmDialog v-model:open="open" title="Delete playlist?" description="Cannot be undone." confirm-label="Delete" :danger="danger" @confirm="onConfirm" />`,
    });
    const w = mount(Host, { attachTo: document.body });
    return { vm: w.vm as unknown as { open: boolean }, confirmed: () => confirmed };
  }

  it("shows the question and description", async () => {
    const { vm } = confirmHost();
    vm.open = true;
    await settle();
    expect(dialog()!.textContent).toContain("Delete playlist?");
    expect(dialog()!.textContent).toContain("Cannot be undone.");
  });

  it("confirming fires confirm once and closes", async () => {
    const { vm, confirmed } = confirmHost();
    vm.open = true;
    await settle();
    btn("Delete").click();
    await settle();
    expect(confirmed()).toBe(1);
    expect(vm.open).toBe(false);
  });

  it("cancelling (button or Escape) never confirms", async () => {
    const { vm, confirmed } = confirmHost();
    vm.open = true;
    await settle();
    btn("Cancel").click();
    await settle();
    expect(vm.open).toBe(false);
    vm.open = true;
    await settle();
    key(dialog()!, "Escape");
    await settle();
    expect(vm.open).toBe(false);
    expect(confirmed()).toBe(0);
  });

  it("styles the confirm button as destructive when danger", async () => {
    const { vm } = confirmHost(true);
    vm.open = true;
    await settle();
    expect(btn("Delete").getAttribute("data-variant")).toBe("danger");
  });
});
