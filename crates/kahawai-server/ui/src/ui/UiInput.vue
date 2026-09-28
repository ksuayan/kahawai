<script setup lang="ts">
import { ref } from "vue";

/**
 * Text input with v-model. Extra attributes (placeholder, type, @keydown,
 * @change…) fall through to the <input>. Width is the caller's business.
 */
withDefaults(defineProps<{ modelValue?: string; size?: "default" | "lg" | "title" }>(), {
  size: "default",
});
const emit = defineEmits<{ (e: "update:modelValue", v: string): void }>();

const sizes = {
  default: "px-2.5 py-1.5 text-[13px]",
  lg: "px-3 py-2 text-sm",
  title: "px-2.5 py-1.5 text-lg font-semibold",
} as const;

const el = ref<HTMLInputElement | null>(null);
defineExpose({ focus: () => el.value?.focus(), el });
</script>

<template>
  <input
    ref="el"
    :value="modelValue"
    class="rounded-md border border-line bg-surface text-fg outline-none placeholder:text-faint focus:border-accent"
    :class="sizes[size]"
    @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
  />
</template>
