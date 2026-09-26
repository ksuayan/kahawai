<script setup lang="ts">
import { SwitchRoot, SwitchThumb } from "reka-ui";

/** On/off toggle (Reka Switch). Use `label` for the visible text. */
defineProps<{ modelValue: boolean; label?: string; disabled?: boolean }>();
const emit = defineEmits<{ (e: "update:modelValue", v: boolean): void }>();
</script>

<template>
  <label class="inline-flex items-center gap-2 text-[13px]" :class="disabled ? 'opacity-45' : 'cursor-pointer'">
    <SwitchRoot
      :model-value="modelValue"
      :disabled="disabled"
      class="relative h-5 w-9 shrink-0 rounded-full bg-active outline-none transition-colors focus-visible:outline-2 focus-visible:outline-accent data-[state=checked]:bg-accent"
      @update:model-value="(v: boolean) => emit('update:modelValue', v)"
    >
      <SwitchThumb
        class="block size-4 translate-x-0.5 rounded-full bg-white shadow transition-transform data-[state=checked]:translate-x-[18px]"
      />
    </SwitchRoot>
    <slot>{{ label }}</slot>
  </label>
</template>
