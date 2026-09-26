<script setup lang="ts">
import UiButton from "./UiButton.vue";
import UiDialog from "./UiDialog.vue";

/** Replaces `window.confirm` (unreliable in the Tauri webview). */
withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    description?: string;
    confirmLabel?: string;
    danger?: boolean;
  }>(),
  { confirmLabel: "Confirm", danger: false },
);
const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  (e: "confirm"): void;
}>();

function confirm(): void {
  emit("confirm");
  emit("update:open", false);
}
</script>

<template>
  <UiDialog :open="open" :title="title" :description="description" @update:open="(v) => emit('update:open', v)">
    <template #footer>
      <UiButton @click="emit('update:open', false)">Cancel</UiButton>
      <UiButton :variant="danger ? 'danger' : 'primary'" data-testid="confirm" @click="confirm">
        {{ confirmLabel }}
      </UiButton>
    </template>
  </UiDialog>
</template>
