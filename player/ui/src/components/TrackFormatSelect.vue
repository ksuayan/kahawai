<script setup lang="ts">
import { computed } from "vue";
import { usePlayerStore } from "../stores/player";
import { validFormatsFor, type StreamFormat, type Track } from "../types";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";

/** Per-track stream format override ("Auto" = let the server ladder decide). */
const props = defineProps<{ track: Track | null; disabled?: boolean; title?: string; triggerClass?: string }>();
const player = usePlayerStore();

const options = computed<UiSelectOption[]>(() => [
  { value: null, label: "Auto" },
  ...(props.track ? validFormatsFor(props.track) : []).map((f) => ({ value: f, label: f.toUpperCase() })),
]);

function onChange(v: string | null): void {
  if (!props.track) return;
  void player.changeTrackFormat(props.track.id, v as StreamFormat | null);
}
</script>

<template>
  <UiSelect
    aria-label="Stream format for this track"
    :model-value="player.activeFormat"
    :options="options"
    :disabled="disabled || !track"
    :title="title"
    :trigger-class="triggerClass"
    @update:model-value="onChange"
  />
</template>
