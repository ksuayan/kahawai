<script setup lang="ts">
import { computed, ref, watch } from "vue";
import UiButton from "./UiButton.vue";
import UiDialog from "./UiDialog.vue";
import UiInput from "./UiInput.vue";

/** Replaces `window.prompt` (which returns nothing in the Tauri webview). */
const props = withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    label?: string;
    placeholder?: string;
    initial?: string;
    confirmLabel?: string;
    maxlength?: number;
  }>(),
  { confirmLabel: "OK", initial: "", maxlength: 120 },
);
const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  (e: "submit", value: string): void;
}>();

const text = ref(props.initial);
watch(
  () => props.open,
  (o) => {
    if (o) text.value = props.initial;
  },
);
const trimmed = computed(() => text.value.trim());

function submit(): void {
  if (!trimmed.value) return;
  emit("submit", trimmed.value);
  emit("update:open", false);
}
</script>

<template>
  <UiDialog :open="open" :title="title" @update:open="(v) => emit('update:open', v)">
    <label class="block">
      <span v-if="label" class="mb-1 block text-xs text-dim">{{ label }}</span>
      <UiInput
        v-model="text"
        class="w-full"
        type="text"
        :placeholder="placeholder"
        :maxlength="maxlength"
        :aria-label="label ?? title"
        @keydown.enter.prevent="submit"
      />
    </label>
    <template #footer>
      <UiButton @click="emit('update:open', false)">Cancel</UiButton>
      <UiButton variant="primary" :disabled="!trimmed" data-testid="submit" @click="submit">
        {{ confirmLabel }}
      </UiButton>
    </template>
  </UiDialog>
</template>
