<script setup lang="ts">
import { computed, ref, useTemplateRef, watch } from "vue";
import { useDspStore } from "../stores/dsp";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import EqEditor from "./EqEditor.vue";

/**
 * The EQ editor as a modal, opened from the transport bar. The editor itself
 * is [EqEditor](./EqEditor.vue), shared with Settings; what belongs here is
 * the dialog chrome and this screen's commit rule: edits apply live so they
 * are audible, OK keeps them, Cancel (or Escape, or the overlay) puts back
 * what was there when it opened. Settings has no such rule — it saves as you
 * go, like the rest of that screen.
 */
const open = defineModel<boolean>("open", { required: true });
const dsp = useDspStore();
const editor = useTemplateRef<InstanceType<typeof EqEditor>>("editor");

let snap: ReturnType<typeof dsp.snapshotEq> | null = null;
watch(
  open,
  (o) => {
    if (o) {
      snap = dsp.snapshotEq();
      editor.value?.clearSelection();
    }
  },
  { immediate: true },
);
function ok(): void {
  snap = null;
  open.value = false;
}
function cancel(): void {
  if (snap) void dsp.restoreEq(snap);
  snap = null;
  open.value = false;
}
/** Escape / overlay click / close all count as Cancel. */
function onOpenChange(v: boolean): void {
  if (v) open.value = true;
  else cancel();
}

const liveNote = computed(() =>
  !dsp.eqEnabled
    ? "EQ is off, so you won't hear changes. Turn it on to preview them."
    : "Changes play live. OK keeps them; Cancel reverts.",
);
</script>

<template>
  <UiDialog :open="open" wide @update:open="onOpenChange" title="Equalizer" description="Drag a point to shape the sound. Double-click the graph to add a band.">
    <EqEditor ref="editor">
      <template #note>
        <p class="m-0 mb-3 text-xs" :class="dsp.eqEnabled ? 'text-dim' : 'text-warn-fg'" data-testid="eq-live-note">
          {{ liveNote }}
        </p>
      </template>
    </EqEditor>
    <template #footer>
      <UiButton data-testid="eq-cancel" @click="cancel">Cancel</UiButton>
      <UiButton variant="primary" data-testid="eq-ok" @click="ok">OK</UiButton>
    </template>
  </UiDialog>
</template>
