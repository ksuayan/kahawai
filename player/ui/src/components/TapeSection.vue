<script setup lang="ts">
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import {
  TAPE_DRIVE_RANGE,
  TAPE_FLUTTER_DEPTH_RANGE,
  TAPE_FLUTTER_RATE_RANGE,
  TAPE_HISS_RANGE,
  TAPE_TRIM_RANGE,
  TAPE_WOW_DEPTH_RANGE,
  TAPE_WOW_RATE_RANGE,
  type TapeEqStandard,
  type TapeQuality,
  type TapeSpeed,
} from "../types";
import UiHint from "../ui/UiHint.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import UiSlider from "../ui/UiSlider.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/**
 * Tape (Stage 1): the on/off switch and grouped controls — tape path,
 * speed & EQ, modulation, noise, quality, mix. A first-order tape color:
 * record/repro EQ, gentle saturation, head bump, wow/flutter/drift and
 * tape hiss. It runs after analog, last among the color stages.
 */
const dsp = useDspStore();
const player = usePlayerStore();

const tp = computed(() => dsp.tape);
const note = computed(() => {
  if (!tp.value.enabled) return null;
  if (player.isExclusive) return "Bypassed: exclusive output sends samples to the DAC untouched.";
  return null;
});

function set(patch: Partial<typeof tp.value>): void {
  void dsp.saveTape({ ...tp.value, ...patch });
}

const speedOptions: UiSelectOption[] = [
  { value: "Ips15", label: "15 ips" },
  { value: "Ips30", label: "30 ips" },
];
const eqOptions: UiSelectOption[] = [
  { value: "nab", label: "NAB" },
  { value: "iec", label: "IEC" },
];
const qualityOptions: UiSelectOption[] = [
  { value: "eco", label: "Eco" },
  { value: "standard", label: "Standard" },
  { value: "high", label: "High" },
];
</script>

<template>
  <div>
    <UiHint>
      Tape color, first stage: record/repro EQ, gentle saturation, head bump, wow/flutter/drift and tape
      hiss — the broad strokes of what a tape path does to the sound. It does not model any specific
      machine. Runs after analog, last among the color stages. Applies to the shared PCM output;
      exclusive (DoP / bit-perfect) output bypasses it.
    </UiHint>
    <UiSwitch
      :model-value="tp.enabled"
      :label="tp.enabled ? 'Tape enabled' : 'Tape disabled'"
      data-testid="tape-toggle"
      @update:model-value="(v) => set({ enabled: v })"
    />
    <p v-if="note" class="m-0 mt-2 text-xs text-dim" role="status" data-testid="tape-note">{{ note }}</p>

    <!-- Only while on: a disabled stage shows its switch and nothing to adjust. -->
    <div v-if="tp.enabled" class="mt-3 flex flex-col gap-4 text-dim" data-testid="tape-controls">
      <div class="flex flex-col gap-3">
        <span class="text-sm font-semibold text-normal">Tape path</span>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Drive</span><span class="tabular-nums" data-testid="tape-drive">{{ tp.drive_db.toFixed(1) }} dB</span>
          </div>
          <UiSlider aria-label="Tape drive" :model-value="tp.drive_db" :min="TAPE_DRIVE_RANGE[0]" :max="TAPE_DRIVE_RANGE[1]" :step="0.5" @update:model-value="(v) => set({ drive_db: v })" />
          <span class="text-xs text-faint">Input gain into the saturation curve.</span>
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Saturation</span><span class="tabular-nums" data-testid="tape-saturation">{{ Math.round(tp.saturation * 100) }}%</span>
          </div>
          <UiSlider aria-label="Tape saturation" :model-value="tp.saturation" :min="0" :max="1" :step="0.01" @update:model-value="(v) => set({ saturation: v })" />
          <span class="text-xs text-faint">Knee sharpness: higher rounds peaks off harder.</span>
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Bias</span><span class="tabular-nums" data-testid="tape-bias">{{ Math.round(tp.bias * 100) }}%</span>
          </div>
          <UiSlider aria-label="Tape bias" :model-value="tp.bias" :min="0" :max="1" :step="0.01" @update:model-value="(v) => set({ bias: v })" />
          <span class="text-xs text-faint">Curve asymmetry: under vs over, for even-order content.</span>
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Output trim</span><span class="tabular-nums" data-testid="tape-trim">{{ tp.output_trim_db.toFixed(1) }} dB</span>
          </div>
          <UiSlider aria-label="Tape output trim" :model-value="tp.output_trim_db" :min="TAPE_TRIM_RANGE[0]" :max="TAPE_TRIM_RANGE[1]" :step="0.5" @update:model-value="(v) => set({ output_trim_db: v })" />
        </div>
      </div>

      <div class="flex flex-col gap-3">
        <span class="text-sm font-semibold text-normal">Speed &amp; EQ</span>
        <label class="flex flex-col gap-1">
          Speed
          <UiSelect aria-label="Tape speed" trigger-class="w-full max-w-xs" :model-value="tp.ips" :options="speedOptions" @update:model-value="(v) => v && set({ ips: v as TapeSpeed })" />
        </label>
        <label class="flex flex-col gap-1">
          EQ standard
          <UiSelect aria-label="Tape EQ standard" trigger-class="w-full max-w-xs" :model-value="tp.eq_standard" :options="eqOptions" @update:model-value="(v) => v && set({ eq_standard: v as TapeEqStandard })" />
          <span class="text-xs text-faint">NAB (1965) is the default; IEC is treble cut only.</span>
        </label>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Head bump</span><span class="tabular-nums" data-testid="tape-head-bump">{{ Math.round(tp.head_bump * 100) }}%</span>
          </div>
          <UiSlider aria-label="Tape head bump" :model-value="tp.head_bump" :min="0" :max="1" :step="0.01" @update:model-value="(v) => set({ head_bump: v })" />
          <span class="text-xs text-faint">The low-frequency lift where the head meets the tape.</span>
        </div>
      </div>

      <div class="flex flex-col gap-3">
        <span class="text-sm font-semibold text-normal">Modulation</span>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Wow rate</span><span class="tabular-nums" data-testid="tape-wow-rate">{{ tp.wow_rate_hz.toFixed(2) }} Hz</span>
          </div>
          <UiSlider aria-label="Tape wow rate" :model-value="tp.wow_rate_hz" :min="TAPE_WOW_RATE_RANGE[0]" :max="TAPE_WOW_RATE_RANGE[1]" :step="0.05" @update:model-value="(v) => set({ wow_rate_hz: v })" />
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Wow depth</span><span class="tabular-nums" data-testid="tape-wow-depth">{{ (tp.wow_depth_pct * 100).toFixed(2) }}%</span>
          </div>
          <UiSlider aria-label="Tape wow depth" :model-value="tp.wow_depth_pct" :min="TAPE_WOW_DEPTH_RANGE[0]" :max="TAPE_WOW_DEPTH_RANGE[1]" :step="0.01" @update:model-value="(v) => set({ wow_depth_pct: v })" />
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Flutter rate</span><span class="tabular-nums" data-testid="tape-flutter-rate">{{ tp.flutter_rate_hz.toFixed(1) }} Hz</span>
          </div>
          <UiSlider aria-label="Tape flutter rate" :model-value="tp.flutter_rate_hz" :min="TAPE_FLUTTER_RATE_RANGE[0]" :max="TAPE_FLUTTER_RATE_RANGE[1]" :step="0.1" @update:model-value="(v) => set({ flutter_rate_hz: v })" />
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Flutter depth</span><span class="tabular-nums" data-testid="tape-flutter-depth">{{ (tp.flutter_depth_pct * 100).toFixed(2) }}%</span>
          </div>
          <UiSlider aria-label="Tape flutter depth" :model-value="tp.flutter_depth_pct" :min="TAPE_FLUTTER_DEPTH_RANGE[0]" :max="TAPE_FLUTTER_DEPTH_RANGE[1]" :step="0.01" @update:model-value="(v) => set({ flutter_depth_pct: v })" />
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Drift</span><span class="tabular-nums" data-testid="tape-drift">{{ Math.round(tp.drift * 100) }}%</span>
          </div>
          <UiSlider aria-label="Tape drift" :model-value="tp.drift" :min="0" :max="1" :step="0.01" @update:model-value="(v) => set({ drift: v })" />
          <span class="text-xs text-faint">Slow pitch wander, separate for left and right.</span>
        </div>
      </div>

      <div class="flex flex-col gap-3">
        <span class="text-sm font-semibold text-normal">Noise</span>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Hiss</span><span class="tabular-nums" data-testid="tape-hiss">{{ tp.hiss_db <= -90 ? "Off" : `${tp.hiss_db.toFixed(0)} dB` }}</span>
          </div>
          <UiSlider aria-label="Tape hiss" :model-value="tp.hiss_db" :min="TAPE_HISS_RANGE[0]" :max="TAPE_HISS_RANGE[1]" :step="1" @update:model-value="(v) => set({ hiss_db: v })" />
          <span class="text-xs text-faint">Shaped tape hiss; all the way down is exactly off.</span>
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex justify-between">
            <span>Asperity</span><span class="tabular-nums" data-testid="tape-asperity">{{ Math.round(tp.asperity * 100) }}%</span>
          </div>
          <UiSlider aria-label="Tape asperity" :model-value="tp.asperity" :min="0" :max="1" :step="0.01" @update:model-value="(v) => set({ asperity: v })" />
          <span class="text-xs text-faint">Signal-dependent grit: louder passages get rougher.</span>
        </div>
      </div>

      <div class="flex flex-col gap-3">
        <span class="text-sm font-semibold text-normal">Quality</span>
        <label class="flex flex-col gap-1">
          Anti-aliasing
          <UiSelect aria-label="Tape anti-aliasing quality" trigger-class="w-full max-w-xs" :model-value="tp.quality" :options="qualityOptions" @update:model-value="(v) => v && set({ quality: v as TapeQuality })" />
          <span class="text-xs text-faint">Eco runs at base rate; Standard 2x and High 4x oversample the saturator.</span>
        </label>
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Dry / wet</span>
          <span class="tabular-nums" data-testid="tape-dry-wet">{{ Math.round(tp.dry_wet * 100) }}%</span>
        </div>
        <UiSlider
          aria-label="Tape dry wet mix"
          :model-value="tp.dry_wet"
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
