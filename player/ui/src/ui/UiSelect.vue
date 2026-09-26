<script setup lang="ts">
import {
  SelectContent,
  SelectIcon,
  SelectItem,
  SelectItemText,
  SelectPortal,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectViewport,
} from "reka-ui";

export interface UiSelectOption {
  /** `null` is a real choice ("Auto", "System default"). */
  value: string | null;
  label: string;
  disabled?: boolean;
}

defineProps<{
  modelValue: string | null;
  options: UiSelectOption[];
  ariaLabel?: string;
  disabled?: boolean;
  placeholder?: string;
}>();
const emit = defineEmits<{ (e: "update:modelValue", v: string | null): void }>();

// Reka's Select does not allow an empty/null item value, so `null` travels
// as a sentinel. Every other value is passed through byte-for-byte (device
// names can carry trailing spaces).
const NONE = "\u0000none";
const toKey = (v: string | null): string => (v === null ? NONE : v);
const fromKey = (k: string): string | null => (k === NONE ? null : k);
</script>

<template>
  <SelectRoot
    :model-value="toKey(modelValue)"
    :disabled="disabled"
    @update:model-value="(k) => emit('update:modelValue', fromKey(String(k)))"
  >
    <SelectTrigger
      :aria-label="ariaLabel"
      class="inline-flex min-w-0 items-center justify-between gap-2 rounded-md border border-line bg-raised px-2.5 py-1.5 text-left text-[13px] text-fg outline-none transition-colors hover:bg-hover focus-visible:border-accent data-[state=open]:border-accent disabled:opacity-45"
    >
      <span class="truncate"><SelectValue :placeholder="placeholder ?? 'Select…'" /></span>
      <SelectIcon class="shrink-0 text-faint">▾</SelectIcon>
    </SelectTrigger>
    <SelectPortal>
      <SelectContent
        position="popper"
        :side-offset="4"
        class="z-50 max-h-[var(--reka-select-content-available-height)] min-w-[var(--reka-select-trigger-width)] overflow-hidden rounded-lg border border-line bg-raised shadow-[0_8px_24px_rgba(0,0,0,0.45)]"
      >
        <SelectViewport class="p-1">
          <SelectItem
            v-for="o in options"
            :key="toKey(o.value)"
            :value="toKey(o.value)"
            :disabled="o.disabled"
            class="relative flex cursor-default select-none items-center rounded-md px-2.5 py-1.5 text-[13px] text-fg outline-none data-[disabled]:opacity-45 data-[highlighted]:bg-hover data-[state=checked]:font-semibold"
          >
            <SelectItemText>{{ o.label }}</SelectItemText>
          </SelectItem>
        </SelectViewport>
      </SelectContent>
    </SelectPortal>
  </SelectRoot>
</template>
