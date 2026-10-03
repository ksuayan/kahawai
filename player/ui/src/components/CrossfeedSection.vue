<script setup lang="ts">
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import {
  CROSSFEED_CUTOFF_RANGE,
  CROSSFEED_FEED_RANGE,
  CROSSFEED_PRESET_INFO,
  CROSSFEED_PRESETS,
  type CrossfeedPreset,
} from "../types";
import UiHint from "../ui/UiHint.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import UiSlider from "../ui/UiSlider.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/**
 * Headphone crossfeed: the on/off switch, the preset menu, and cutoff/feed
 * sliders. Choosing a preset fills the sliders with its values; moving a
 * slider switches the menu to Custom (the convention bs2b-based players use).
 */
const dsp = useDspStore();
const player = usePlayerStore();

const presetOptions: UiSelectOption[] = CROSSFEED_PRESETS.map((p) => ({ value: p, label: CROSSFEED_PRESET_INFO[p].label }));
const cf = computed(() => dsp.crossfeed);
/** What is in effect: the preset's values, or Custom's own. */
const params = computed<[number, number]>(
  () => CROSSFEED_PRESET_INFO[cf.value.preset].params ?? [cf.value.cutoff_hz, cf.value.feed_db],
);
const note = computed(() => {
  if (!cf.value.enabled) return null;
  if (player.isExclusive) return "Bypassed: exclusive output sends samples to the DAC untouched.";
  return null;
});

function setPreset(p: CrossfeedPreset): void {
  const preset = CROSSFEED_PRESET_INFO[p].params;
  void dsp.saveCrossfeed({
    ...cf.value,
    preset: p,
    ...(preset ? { cutoff_hz: preset[0], feed_db: preset[1] } : {}),
  });
}

function setParam(patch: { cutoff_hz?: number; feed_db?: number }): void {
  const [cutoff_hz, feed_db] = params.value;
  void dsp.saveCrossfeed({ ...cf.value, cutoff_hz, feed_db, ...patch, preset: "custom" });
}
</script>

<template>
  <div>
    <UiHint>
      For headphones: blends a little of each channel into the other, as your ears would hear a pair of
      speakers, so hard-panned recordings are less tiring over a long session. Stereo only. Applies to the
      shared PCM output; exclusive (DoP / bit-perfect) output bypasses it.
    </UiHint>
    <UiSwitch
      :model-value="cf.enabled"
      :label="cf.enabled ? 'Crossfeed enabled' : 'Crossfeed disabled'"
      data-testid="crossfeed-toggle"
      @update:model-value="(v) => dsp.saveCrossfeed({ ...cf, enabled: v })"
    />
    <p v-if="note" class="m-0 mt-2 text-xs text-dim" role="status" data-testid="crossfeed-note">{{ note }}</p>

    <!-- Only while on: a disabled stage shows its switch and nothing to adjust. -->
    <div v-if="cf.enabled" class="mt-3 flex flex-col gap-3 text-dim" data-testid="crossfeed-controls">
      <label class="flex flex-col gap-1">
        Preset
        <UiSelect
          aria-label="Crossfeed preset"
          trigger-class="w-full max-w-xs"
          :model-value="cf.preset"
          :options="presetOptions"
          @update:model-value="(v) => v && setPreset(v as CrossfeedPreset)"
        />
        <span class="text-xs text-faint" data-testid="crossfeed-blurb">{{ CROSSFEED_PRESET_INFO[cf.preset].blurb }}</span>
      </label>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Cutoff</span><span class="tabular-nums" data-testid="crossfeed-cutoff">{{ Math.round(params[0]) }} Hz</span>
        </div>
        <UiSlider
          aria-label="Crossfeed cutoff"
          :model-value="params[0]"
          :min="CROSSFEED_CUTOFF_RANGE[0]"
          :max="CROSSFEED_CUTOFF_RANGE[1]"
          :step="10"
          @update:model-value="(v) => setParam({ cutoff_hz: v })"
        />
      </div>

      <div class="flex flex-col gap-1">
        <div class="flex justify-between">
          <span>Feed</span><span class="tabular-nums" data-testid="crossfeed-feed">{{ params[1].toFixed(1) }} dB</span>
        </div>
        <UiSlider
          aria-label="Crossfeed feed"
          :model-value="params[1]"
          :min="CROSSFEED_FEED_RANGE[0]"
          :max="CROSSFEED_FEED_RANGE[1]"
          :step="0.5"
          @update:model-value="(v) => setParam({ feed_db: v })"
        />
      </div>
    </div>
  </div>
</template>
