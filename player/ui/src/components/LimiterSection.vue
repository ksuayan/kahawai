<script setup lang="ts">
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import UiHint from "../ui/UiHint.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/**
 * Look-ahead limiter: the on/off switch and a gain-reduction meter. The meter
 * reads the peak-held reduction the engine reports (about four times a
 * second), so a duck lasting a few milliseconds is still visible.
 */
const dsp = useDspStore();
const player = usePlayerStore();

/** Full-scale of the meter. Past this the EQ/gain settings are the problem. */
const FULL_SCALE_DB = 12;
/** Green below the first, yellow below the second, red above it. */
const GENTLE_DB = 3;
const HEAVY_DB = 6;

const bypassed = computed(() => player.isExclusive);
const gr = computed(() => player.limiterGrDb);
/** Working = the engine is reporting a live reading for the shared path. */
const reading = computed(() => (dsp.limiterEnabled && !bypassed.value ? gr.value : null));
const pct = computed(() => Math.min(100, Math.max(0, ((reading.value ?? 0) / FULL_SCALE_DB) * 100)));
const state = computed<"ok" | "warn" | "bad">(() => {
  const db = reading.value ?? 0;
  return db >= HEAVY_DB ? "bad" : db >= GENTLE_DB ? "warn" : "ok";
});
const note = computed(() => {
  if (!dsp.limiterEnabled) return "Off: peaks over the ceiling are soft-clipped instead, which is less transparent.";
  if (bypassed.value) return "Bypassed: exclusive output sends samples to the DAC untouched.";
  if (reading.value === null) return "Nothing playing on the shared output.";
  if (reading.value >= HEAVY_DB) return "Working hard. Lower the EQ boost or the loudness target to get it out of the way.";
  if (reading.value >= GENTLE_DB) return "Holding back real peaks.";
  return "Barely working, which is where you want it.";
});
</script>

<template>
  <div>
    <UiHint>
      Catches a peak before it arrives and eases the gain down instead of clipping it. Applies to the
      shared PCM output only; exclusive (DoP / bit-perfect) output bypasses it.
    </UiHint>
    <UiSwitch
      :model-value="dsp.limiterEnabled"
      :label="dsp.limiterEnabled ? 'Limiter enabled' : 'Limiter disabled'"
      @update:model-value="(v) => dsp.saveLimiterEnabled(v)"
    />

    <div class="mt-3 rounded-lg border border-line bg-surface p-3" data-testid="limiter-meter">
      <div class="mb-1 flex flex-wrap items-baseline justify-between gap-2">
        <span class="heading-3">Gain reduction</span>
        <span
          v-if="reading !== null"
          class="font-semibold tabular-nums"
          :class="state === 'ok' ? 'text-ok' : state === 'warn' ? 'text-warn-fg' : 'text-danger-fg'"
          data-testid="limiter-gr"
          :data-state="state"
        >−{{ reading.toFixed(1) }} dB</span>
        <span v-else class="text-xs text-dim" data-testid="limiter-gr-idle">—</span>
      </div>
      <div
        class="relative h-2 overflow-hidden rounded-sm bg-active"
        :class="reading === null ? 'opacity-40' : ''"
        role="img"
        :aria-label="
          reading === null
            ? 'Gain reduction meter: no reading'
            : `Gain reduction ${reading.toFixed(1)} dB of ${FULL_SCALE_DB} dB`
        "
      >
        <div
          class="absolute left-0 top-0 h-full rounded-sm transition-[width] duration-150"
          :class="state === 'ok' ? 'bg-ok' : state === 'warn' ? 'bg-warn' : 'bg-danger'"
          :style="{ width: `${pct}%` }"
          data-testid="limiter-bar"
        />
      </div>
      <div class="mt-1 flex justify-between text-[11px] text-faint tabular-nums">
        <span>0</span><span>−{{ GENTLE_DB }}</span><span>−{{ HEAVY_DB }}</span><span>−{{ FULL_SCALE_DB }} dB</span>
      </div>
      <p class="m-0 mt-2 text-xs text-dim" role="status" data-testid="limiter-note">{{ note }}</p>
    </div>
  </div>
</template>
