<script setup lang="ts">
import { computed, ref } from "vue";
import { usePlayerStore } from "../stores/player";
import { formatDuration } from "../types";
import UiSlider from "../ui/UiSlider.vue";

/**
 * Elapsed time, seek slider (with the received-data fill behind it) and duration.
 *
 * While the user drags, the slider and the elapsed label follow the pointer
 * (`scrubbing`), not the engine; the seek is sent once on release (or per key
 * step). Keyboard steps are 1 s (Shift = 10 s).
 */
const props = withDefaults(defineProps<{ disabled?: boolean; size?: "sm" | "md" }>(), {
  disabled: false,
  size: "sm",
});

const player = usePlayerStore();
const scrubbing = ref(false);
const scrubValue = ref(0);

const duration = computed(() => player.durationMs ?? 0);
const shown = computed(() => (scrubbing.value ? scrubValue.value : player.positionMs));
const isDisabled = computed(() => props.disabled || duration.value <= 0);

function onUpdate(v: number): void {
  scrubbing.value = true;
  scrubValue.value = v;
}

function onCommit(v: number): void {
  scrubValue.value = v;
  scrubbing.value = false;
  void player.seekTo(v);
}

const timeClass = computed(() =>
  props.size === "md"
    ? "min-w-12 text-center text-xs tabular-nums text-dim"
    : "min-w-11 text-center text-[11px] tabular-nums text-dim",
);
</script>

<template>
  <div class="flex items-center" :class="size === 'md' ? 'gap-3' : 'gap-2'">
    <span :class="timeClass" data-testid="elapsed">{{ formatDuration(shown) }}</span>
    <UiSlider
      class="flex-1"
      aria-label="Seek"
      :model-value="shown"
      :min="0"
      :max="duration"
      :step="1000"
      :disabled="isDisabled"
      :buffered="player.bufferedMs"
      @update:model-value="onUpdate"
      @commit="onCommit"
    />
    <span :class="timeClass" data-testid="duration">{{ formatDuration(duration || null) }}</span>
  </div>
</template>
