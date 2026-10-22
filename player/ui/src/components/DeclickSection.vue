<script setup lang="ts">
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import { DECLICK_MAX_WIDTH_RANGE, DECLICK_SENSITIVITY_RANGE } from "../types";
import UiHint from "../ui/UiHint.vue";
import UiSlider from "../ui/UiSlider.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/**
 * De-click: the on/off switch, sensitivity, max-width and dry/wet sliders.
 * Repairs short impulsive clicks in vinyl transfers by interpolation; wider
 * defects are left alone. First in the DSP chain.
 */
const dsp = useDspStore();
const player = usePlayerStore();

const dc = computed(() => dsp.declick);
const note = computed(() => {
  if (!dc.value.enabled) return null;
  if (player.isExclusive) return "Bypassed: exclusive output sends samples to the DAC untouched.";
  return null;
});

function set(patch: Partial<typeof dc.value>): void {
  void dsp.saveDeclick({ ...dc.value, ...patch });
}
</script>

<template>
  <div>
    <UiHint>
      For vinyl transfers: finds short impulsive clicks and repairs them by interpolation. Defects wider
      than the max width are left alone (groove damage, not clicks). Runs first in the chain, on the raw
      transfer. Applies to the shared PCM output; exclusive (DoP / bit-perfect) output bypasses it.
    </UiHint>
    <UiSwitch
      :model-value="dc.enabled"
      :label="dc.enabled ? 'De-click enabled' : 'De-click disabled'"
      data-testid="declick-toggle"
      @update:model-value="(v) => set({ enabled: v })"
    />
    <p v-if="note" class="m-0 mt-2 text-xs text-dim" role="status" data-testid="declick-note">{{ note }}</p>

    <!-- Only while on: a disabled stage shows its switch and nothing to adjust. -->
    <div v-if="dc.enabled" class="mt-3 flex flex-col gap-3 text-dim" data-testid="declick-controls">
      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Sensitivity</span>
          <span class="tabular-nums" data-testid="declick-sensitivity">{{ dc.sensitivity.toFixed(2) }}</span>
        </div>
        <UiSlider
          aria-label="De-click sensitivity"
          :model-value="dc.sensitivity"
          :min="DECLICK_SENSITIVITY_RANGE[0]"
          :max="DECLICK_SENSITIVITY_RANGE[1]"
          :step="0.01"
          @update:model-value="(v) => set({ sensitivity: v })"
        />
        <span class="text-xs text-faint">Gentle (left) misses quiet clicks; aggressive (right) may mark sharp musical transients.</span>
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Max click width</span>
          <span class="tabular-nums" data-testid="declick-max-width">{{ dc.max_width_ms.toFixed(1) }} ms</span>
        </div>
        <UiSlider
          aria-label="De-click max click width"
          :model-value="dc.max_width_ms"
          :min="DECLICK_MAX_WIDTH_RANGE[0]"
          :max="DECLICK_MAX_WIDTH_RANGE[1]"
          :step="0.1"
          @update:model-value="(v) => set({ max_width_ms: v })"
        />
        <span class="text-xs text-faint">Defects wider than this are left alone.</span>
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Dry / wet</span>
          <span class="tabular-nums" data-testid="declick-dry-wet">{{ Math.round(dc.dry_wet * 100) }}%</span>
        </div>
        <UiSlider
          aria-label="De-click dry wet mix"
          :model-value="dc.dry_wet"
          :min="0"
          :max="1"
          :step="0.01"
          @update:model-value="(v) => set({ dry_wet: v })"
        />
        <span class="text-xs text-faint">The A/B control: 0% is the unprocessed transfer.</span>
      </div>
    </div>
  </div>
</template>
