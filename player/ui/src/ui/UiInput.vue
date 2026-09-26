<script setup lang="ts">
import { ref } from "vue";

/** Text input with v-model. Extra attributes (placeholder, type, @keydown…) fall through. */
defineProps<{ modelValue?: string }>();
const emit = defineEmits<{ (e: "update:modelValue", v: string): void }>();

const el = ref<HTMLInputElement | null>(null);
defineExpose({ focus: () => el.value?.focus(), el });
</script>

<template>
  <input
    ref="el"
    :value="modelValue"
    class="rounded-md border border-line bg-raised px-2.5 py-1.5 text-[13px] text-fg outline-none placeholder:text-faint focus:border-accent"
    @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
  />
</template>
