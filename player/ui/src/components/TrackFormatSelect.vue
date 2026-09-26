<script setup lang="ts">
import { computed } from "vue";
import { usePlayerStore } from "../stores/player";
import { validFormatsFor, type StreamFormat, type Track } from "../types";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";

/**
 * "Stream this track as…": a one-track override.
 *
 * Auto (the default) means the server/Settings default decides; the format
 * actually in use is shown by the chain badge and in the tooltip. Choosing a
 * format forces it for this track only; choosing Auto clears the override.
 */
const props = defineProps<{ track: Track | null; disabled?: boolean; title?: string; triggerClass?: string }>();
const player = usePlayerStore();

const options = computed<UiSelectOption[]>(() => [
  { value: null, label: "Auto" },
  ...(props.track ? validFormatsFor(props.track) : []).map((f) => ({ value: f, label: f.toUpperCase() })),
]);

const override = computed(() => player.formatOverride(props.track?.id));

const tooltip = computed(() => {
  if (props.title) return props.title;
  const now = player.activeFormat ? ` (playing as ${player.activeFormat.toUpperCase()})` : "";
  return override.value
    ? `This track is forced to ${override.value.toUpperCase()}. Choose Auto to use the default from Settings.`
    : `Auto: uses the default from Settings${now}. Pick a format to override it for this track only.`;
});

function onChange(v: string | null): void {
  if (!props.track) return;
  void player.changeTrackFormat(props.track.id, v as StreamFormat | null);
}
</script>

<template>
  <UiSelect
    aria-label="Stream format for this track"
    :model-value="override"
    :options="options"
    :disabled="disabled || !track"
    :title="tooltip"
    :trigger-class="triggerClass"
    @update:model-value="onChange"
  />
</template>
