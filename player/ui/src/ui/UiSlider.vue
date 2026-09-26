<script setup lang="ts">
import { computed } from "vue";
import { SliderRange, SliderRoot, SliderThumb, SliderTrack } from "reka-ui";

/**
 * Horizontal slider (Reka Slider).
 *
 * `update:modelValue` fires continuously while dragging / on every key step;
 * `commit` fires once when the user lets go (or a key press lands). Use the
 * pair for "scrub freely, apply on release" (seek) or apply on `update` for
 * live controls (volume).
 */
const props = withDefaults(
  defineProps<{
    modelValue: number;
    min?: number;
    max?: number;
    step?: number;
    disabled?: boolean;
    ariaLabel?: string;
    /** Optional secondary fill (same units as the value), e.g. data buffered ahead of a playhead. */
    buffered?: number | null;
  }>(),
  { min: 0, max: 100, step: 1, disabled: false },
);
const emit = defineEmits<{
  (e: "update:modelValue", v: number): void;
  (e: "commit", v: number): void;
}>();

// Reka needs max > min; a track with no known duration gets a 1-wide range.
const safeMax = computed(() => Math.max(props.max, props.min + 1));
const bufferedPct = computed(() =>
  props.buffered == null ? null : Math.min(100, Math.max(0, ((props.buffered - props.min) / (safeMax.value - props.min)) * 100)),
);
const value = computed(() => [Math.min(Math.max(props.modelValue, props.min), safeMax.value)]);
</script>

<template>
  <SliderRoot
    :model-value="value"
    :min="min"
    :max="safeMax"
    :step="step"
    :disabled="disabled"
    class="group relative flex h-5 touch-none select-none items-center data-[disabled]:opacity-45"
    @update:model-value="(v) => v && emit('update:modelValue', v[0])"
    @value-commit="(v) => v && emit('commit', v[0])"
  >
    <SliderTrack class="relative h-1 grow overflow-hidden rounded-full bg-active">
      <div
        v-if="bufferedPct !== null"
        class="absolute h-full rounded-full bg-faint/50"
        :style="{ width: `${bufferedPct}%` }"
        data-testid="buffered"
      />
      <SliderRange class="absolute h-full rounded-full bg-dim group-hover:bg-accent group-data-[disabled]:bg-dim" />
    </SliderTrack>
    <SliderThumb
      :aria-label="ariaLabel"
      class="block size-3 rounded-full bg-fg shadow outline-none transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent group-hover:bg-accent"
    />
  </SliderRoot>
</template>
