<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { usePlayerStore } from "../stores/player";
import UiSlider from "../ui/UiSlider.vue";

/**
 * Live volume control (0–100 %). Every step goes to the engine straight away.
 * While the user is adjusting, engine state events that predate the latest
 * input are ignored so the thumb is not pulled back mid-drag.
 */
const player = usePlayerStore();
const value = ref(Math.round(player.volume * 100));
let touchedAt = 0;

watch(
  () => player.volume,
  (v) => {
    if (Date.now() - touchedAt < 600) return;
    value.value = Math.round(v * 100);
  },
);

const volumeTitle = computed(() =>
  player.isBitPerfect
    ? "Bit-perfect output sends the samples untouched: use your DAC's volume control"
    : player.isDopExclusive
      ? "Volume is ignored on exclusive DoP output"
      : "Volume",
);

function onUpdate(v: number): void {
  touchedAt = Date.now();
  value.value = v;
  void player.changeVolume(v / 100);
}
</script>

<template>
  <UiSlider
    aria-label="Volume"
    :model-value="value"
    :min="0"
    :max="100"
    :step="1"
    :class="player.isExclusive && 'opacity-40'"
    :title="volumeTitle"
    @update:model-value="onUpdate"
  />
</template>
