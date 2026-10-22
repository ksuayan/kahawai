<script setup lang="ts">
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import {
  DEHISS_AMOUNT_RANGE,
  DEHISS_ATTACK_RANGE,
  DEHISS_RELEASE_RANGE,
  DEHISS_THRESHOLD_RANGE,
  DEHISS_TRIM_RANGE,
} from "../types";
import UiHint from "../ui/UiHint.vue";
import UiSlider from "../ui/UiSlider.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/**
 * De-hiss: the on/off switch, threshold/amount/attack/release sliders, the
 * per-band trims, and dry/wet. A 4-band downward expander that turns the
 * quiet parts down; it does not remove noise, it lowers the level where
 * the signal drops into the hiss. Second in the DSP chain, after de-click.
 */
const dsp = useDspStore();
const player = usePlayerStore();

const dh = computed(() => dsp.dehiss);
const note = computed(() => {
  if (!dh.value.enabled) return null;
  if (player.isExclusive) return "Bypassed: exclusive output sends samples to the DAC untouched.";
  return null;
});

function set(patch: Partial<typeof dh.value>): void {
  void dsp.saveDehiss({ ...dh.value, ...patch });
}

function setTrim(band: number, v: number): void {
  const trim_db = [...dh.value.trim_db] as [number, number, number, number];
  trim_db[band] = v;
  set({ trim_db });
}

const BAND_NAMES = ["Low", "Low-mid", "High-mid", "High"];
</script>

<template>
  <div>
    <UiHint>
      For tapes and vinyl: a 4-band expander that turns the quiet parts down where the signal drops into the
      hiss. It lowers the level of the hiss between notes; it does not remove noise from the music itself.
      Runs second in the chain, after de-click. Applies to the shared PCM output; exclusive (DoP /
      bit-perfect) output bypasses it.
    </UiHint>
    <UiSwitch
      :model-value="dh.enabled"
      :label="dh.enabled ? 'De-hiss enabled' : 'De-hiss disabled'"
      data-testid="dehiss-toggle"
      @update:model-value="(v) => set({ enabled: v })"
    />
    <p v-if="note" class="m-0 mt-2 text-xs text-dim" role="status" data-testid="dehiss-note">{{ note }}</p>

    <!-- Only while on: a disabled stage shows its switch and nothing to adjust. -->
    <div v-if="dh.enabled" class="mt-3 flex flex-col gap-3 text-dim" data-testid="dehiss-controls">
      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Threshold</span>
          <span class="tabular-nums" data-testid="dehiss-threshold">{{ dh.threshold_db.toFixed(0) }} dBFS</span>
        </div>
        <UiSlider
          aria-label="De-hiss threshold"
          :model-value="dh.threshold_db"
          :min="DEHISS_THRESHOLD_RANGE[0]"
          :max="DEHISS_THRESHOLD_RANGE[1]"
          :step="1"
          @update:model-value="(v) => set({ threshold_db: v })"
        />
        <span class="text-xs text-faint">Bands below their threshold (plus trim) start turning down.</span>
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Amount</span>
          <span class="tabular-nums" data-testid="dehiss-amount">{{ dh.amount_db.toFixed(0) }} dB</span>
        </div>
        <UiSlider
          aria-label="De-hiss amount"
          :model-value="dh.amount_db"
          :min="DEHISS_AMOUNT_RANGE[0]"
          :max="DEHISS_AMOUNT_RANGE[1]"
          :step="0.5"
          @update:model-value="(v) => set({ amount_db: v })"
        />
        <span class="text-xs text-faint">Maximum reduction per band.</span>
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Attack</span>
          <span class="tabular-nums" data-testid="dehiss-attack">{{ dh.attack_ms.toFixed(1) }} ms</span>
        </div>
        <UiSlider
          aria-label="De-hiss attack"
          :model-value="dh.attack_ms"
          :min="DEHISS_ATTACK_RANGE[0]"
          :max="DEHISS_ATTACK_RANGE[1]"
          :step="0.1"
          @update:model-value="(v) => set({ attack_ms: v })"
        />
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Release</span>
          <span class="tabular-nums" data-testid="dehiss-release">{{ dh.release_ms.toFixed(0) }} ms</span>
        </div>
        <UiSlider
          aria-label="De-hiss release"
          :model-value="dh.release_ms"
          :min="DEHISS_RELEASE_RANGE[0]"
          :max="DEHISS_RELEASE_RANGE[1]"
          :step="5"
          @update:model-value="(v) => set({ release_ms: v })"
        />
        <span class="text-xs text-faint">Longer releases breathe less but dull note endings.</span>
      </div>

      <div class="flex flex-col gap-2">
        <span>Band trims <span class="text-xs text-faint">(threshold offset per band)</span></span>
        <div v-for="(name, i) in BAND_NAMES" :key="name" class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span class="text-sm">{{ name }}</span>
            <span class="tabular-nums text-sm" :data-testid="`dehiss-trim-${i}`">{{ dh.trim_db[i].toFixed(1) }} dB</span>
          </div>
          <UiSlider
            :aria-label="`De-hiss ${name} trim`"
            :model-value="dh.trim_db[i]"
            :min="DEHISS_TRIM_RANGE[0]"
            :max="DEHISS_TRIM_RANGE[1]"
            :step="0.5"
            @update:model-value="(v) => setTrim(i, v)"
          />
        </div>
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Dry / wet</span>
          <span class="tabular-nums" data-testid="dehiss-dry-wet">{{ Math.round(dh.dry_wet * 100) }}%</span>
        </div>
        <UiSlider
          aria-label="De-hiss dry wet mix"
          :model-value="dh.dry_wet"
          :min="0"
          :max="1"
          :step="0.01"
          @update:model-value="(v) => set({ dry_wet: v })"
        />
        <span class="text-xs text-faint">The A/B control: 0% is the unprocessed signal.</span>
      </div>
    </div>
  </div>
</template>
