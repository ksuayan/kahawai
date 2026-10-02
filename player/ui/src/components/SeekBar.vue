<script setup lang="ts">
import { computed, ref } from "vue";
import { useAudiobooksStore } from "../stores/audiobooks";
import { usePlayerStore } from "../stores/player";
import { useRadioStore } from "../stores/radio";
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
const books = useAudiobooksStore();
const radio = useRadioStore();
/** A radio station has no timeline: no seeking, no duration. */
const live = computed(() => radio.isPlaying);
/** A book plays as one piece: the bar spans the whole book, not the file. */
const book = computed(() => (books.isActive ? books.active : null));
const scrubbing = ref(false);
const scrubValue = ref(0);

const duration = computed(() => (book.value ? book.value.duration_ms : (player.durationMs ?? 0)));
const shown = computed(() => (scrubbing.value ? scrubValue.value : book.value ? books.offsetMs : player.positionMs));
const isDisabled = computed(() => props.disabled || live.value || duration.value <= 0);

function onUpdate(v: number): void {
  scrubbing.value = true;
  scrubValue.value = v;
}

function onCommit(v: number): void {
  scrubValue.value = v;
  scrubbing.value = false;
  void (book.value ? books.seekToOffset(v) : player.seekTo(v));
}

const timeClass = computed(() =>
  props.size === "md"
    ? "min-w-12 text-center text-xs tabular-nums text-dim"
    : "min-w-11 text-center text-[11px] tabular-nums text-dim",
);
</script>

<template>
  <div class="flex items-center" :class="size === 'md' ? 'gap-3' : 'gap-2'">
    <span :class="timeClass" data-testid="elapsed">{{ live ? "LIVE" : formatDuration(shown) }}</span>
    <UiSlider
      class="flex-1"
      aria-label="Seek"
      :model-value="shown"
      :min="0"
      :max="duration"
      :step="1000"
      :disabled="isDisabled"
      :buffered="book ? null : player.bufferedMs"
      @update:model-value="onUpdate"
      @commit="onCommit"
    />
    <span :class="timeClass" data-testid="duration">{{ live ? "" : formatDuration(duration || null) }}</span>
  </div>
</template>
