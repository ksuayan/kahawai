<script setup lang="ts">
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import { useSettingsStore } from "../stores/settings";
import type { QualityMode } from "../types";
import UiBadge from "../ui/UiBadge.vue";
import UiHint from "../ui/UiHint.vue";
import SettingsSection from "./SettingsSection.vue";

/**
 * The one top-level choice: Best quality or Compatible. Below it, a live
 * status line says what is actually happening on this output right now, and
 * a legend shows which of the user's own processing is on, off, or bypassed
 * by exclusive output.
 */
const settings = useSettingsStore();
const player = usePlayerStore();
const dsp = useDspStore();

const modes: { value: QualityMode; title: string; body: string }[] = [
  {
    value: "best",
    title: "Best quality",
    body: "Bit-perfect at each file's own sample rate, and native DSD, when your output can do it. Falls back automatically.",
  },
  {
    value: "compatible",
    title: "Compatible",
    body: "Shared output that works everywhere. EQ, loudness and volume always apply, and DSD is converted.",
  },
];

const blockers = computed(() => player.raw?.exclusive_blockers ?? []);
const caps = computed(() => dsp.dop?.capabilities ?? null);

const status = computed<{ tone: "ok" | "warn" | "info"; text: string } | null>(() => {
  if (settings.qualityMode === "compatible") {
    return { tone: "info", text: "Shared output. EQ, loudness and volume all apply." };
  }
  if (player.isBitPerfect) {
    const hz = player.raw?.output_rate_hz;
    return { tone: "ok", text: `Playing bit-perfect${hz ? ` at ${Math.round(hz / 100) / 10} kHz` : ""}.` };
  }
  if (player.isDopExclusive) return { tone: "ok", text: "Playing native DSD over DoP." };
  if (caps.value && !caps.value.external_dac) {
    return {
      tone: "info",
      text: "This output isn't an external DAC, so Best quality stays on shared output rather than taking it over.",
    };
  }
  if (blockers.value.length) {
    return {
      tone: "warn",
      text: `Paused: ${blockers.value.join(" and ")} ${blockers.value.length === 1 ? "is" : "are"} on. Turn ${
        blockers.value.length === 1 ? "it" : "them"
      } off for bit-perfect output.`,
    };
  }
  if (caps.value?.external_dac) {
    return { tone: "ok", text: "Ready: tracks will play bit-perfect at their own sample rate." };
  }
  return null;
});

type State = "bypassed" | "on" | "off";
const stateOf = (name: string): State =>
  player.isExclusive ? "bypassed" : blockers.value.includes(name) ? "on" : "off";

// In chain order (crossfeed runs first). Names match the engine's blockers.
const processing = computed(() => [
  { name: "Crossfeed", state: stateOf("Crossfeed") },
  { name: "EQ", state: stateOf("EQ") },
  { name: "Loudness", state: stateOf("Loudness") },
  { name: "Analog", state: stateOf("Analog") },
  { name: "Volume", state: stateOf("Volume") },
]);

const stateText: Record<State, string> = { bypassed: "bypassed", on: "on", off: "off" };
const toneClass = { ok: "text-ok", warn: "text-warn", info: "text-dim" } as const;
</script>

<template>
  <SettingsSection title="Sound quality">
    <UiHint>
      One choice for how music is played. Best quality negotiates each file against your output device for you; you
      shouldn't need to change anything else.
    </UiHint>
    <div class="my-2 grid grid-cols-1 gap-2 min-[560px]:grid-cols-2" role="radiogroup" aria-label="Sound quality">
      <button
        v-for="m in modes"
        :key="m.value"
        type="button"
        role="radio"
        :aria-checked="settings.qualityMode === m.value"
        class="rounded-lg border p-3 text-left transition-colors focus-visible:outline-2 focus-visible:outline-accent"
        :class="settings.qualityMode === m.value ? 'border-accent bg-accent/10' : 'border-line bg-raised hover:border-dim'"
        :data-testid="`quality-${m.value}`"
        @click="settings.saveQualityMode(m.value)"
      >
        <span class="block font-semibold">{{ m.title }}</span>
        <span class="mt-0.5 block text-xs text-dim">{{ m.body }}</span>
      </button>
    </div>

    <p v-if="status" class="m-0 mb-2 text-sm" :class="toneClass[status.tone]" role="status" data-testid="quality-status">
      {{ status.text }}
    </p>

    <div class="flex flex-wrap items-center gap-1.5 text-xs" data-testid="processing-legend">
      <span class="text-dim">Your processing:</span>
      <UiBadge
        v-for="p in processing"
        :key="p.name"
        :variant="p.state === 'on' ? 'accent' : 'default'"
        :class="p.state === 'bypassed' ? 'line-through opacity-50' : p.state === 'off' ? 'opacity-60' : ''"
        :data-state="p.state"
        :data-testid="`processing-${p.name.toLowerCase()}`"
        :title="
          p.state === 'bypassed'
            ? `${p.name} is bypassed while exclusive output is playing`
            : p.state === 'on'
              ? `${p.name} is on, so exclusive output is held back`
              : `${p.name} is off`
        "
      >{{ p.name }} {{ stateText[p.state] }}</UiBadge>
    </div>
  </SettingsSection>
</template>
