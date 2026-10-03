<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import { useSignalStore } from "../stores/signal";
import { audioPathLabel, depthLink, dsdName, fmtKHz, isDsd, rateLink, type Link } from "../signalPath";
import { mqaLabel } from "../types";
import UiBadge from "../ui/UiBadge.vue";

/**
 * Live, side by side: the media file on the left, the output device on the
 * right, and what happens between them in the middle. The device column is
 * read from the OS every second and on every playback change, so it shows
 * what the DAC is actually running, not what the player intended.
 */
const player = usePlayerStore();
const dsp = useDspStore();
const signal = useSignalStore();

let stop: (() => void) | undefined;
onMounted(() => {
  stop = signal.watch(1000);
});
onBeforeUnmount(() => stop?.());
// Snap to the new state immediately when the track, path or rate changes.
watch(
  () => [player.raw?.track?.id, player.raw?.output_path, player.raw?.output_rate_hz, player.raw?.status],
  () => void signal.refresh(),
);

const track = computed(() => player.currentTrack);
const live = computed(() => signal.live);
const caps = computed(() => dsp.dop?.capabilities ?? null);

// --- media file ---------------------------------------------------------------
const fileDepth = computed(() => {
  const t = track.value;
  if (!t) return "—";
  if (isDsd(t)) return "1-bit (DSD)";
  return t.bit_depth ? `${t.bit_depth}-bit` : "—";
});
const fileRate = computed(() => {
  const t = track.value;
  if (!t?.sample_rate) return "—";
  if (isDsd(t)) return `${dsdName(t.sample_rate)} · ${(t.sample_rate / 1e6).toFixed(4)} MHz`;
  return fmtKHz(t.sample_rate);
});
const fileFormat = computed(() => audioPathLabel(track.value, player, dsp));

// --- output device --------------------------------------------------------------
const outDepth = computed(() => {
  const l = live.value;
  if (!l?.bit_depth) return "—";
  if (l.float) return `${l.bit_depth}-bit float`;
  // A 32-bit integer slot carries a 24-bit word (16-bit files stay 16 in it).
  return l.exclusive && l.bit_depth === 32 ? "32-bit slot · 24-bit audio" : `${l.bit_depth}-bit`;
});
const outRate = computed(() => (live.value?.rate_hz ? fmtKHz(live.value.rate_hz) : "—"));

/** What the device is being fed. */
const outMode = computed(() => {
  const t = track.value;
  if (!t || player.raw?.status === "stopped") return "Idle";
  if (player.isDopExclusive) return "DSD over PCM (DoP) · exclusive";
  if (player.isBitPerfect) return t.mqa ? "MQA stream, untouched · exclusive" : "PCM · bit-perfect · exclusive";
  return t.mqa ? "PCM · shared mixer (MQA not decoded)" : "PCM · shared mixer";
});

const rate = computed<Link>(() => rateLink(track.value, live.value?.rate_hz ?? null, player.isDopExclusive));
const depth = computed<Link>(() =>
  depthLink(track.value, live.value?.bit_depth ?? 0, player.isExclusive, live.value?.float ?? false),
);

const linkTone: Record<Link["state"], string> = {
  match: "text-ok",
  carried: "text-ok",
  convert: "text-warn-fg",
  none: "text-faint",
};
const linkMark: Record<Link["state"], string> = { match: "=", carried: "→", convert: "≠", none: "·" };

// --- DSP load -----------------------------------------------------------------
const dspLoad = computed(() => player.raw?.dsp_load ?? 0);
const dspStages = computed(() => player.raw?.dsp_stage_load ?? []);
const underruns = computed(() => player.raw?.underruns ?? 0);
/** The meter is live only while the PCM chain is actually processing. */
const showLoad = computed(
  () => player.raw?.status === "playing" && player.raw?.output_path === "pcm-shared",
);
const loadPct = computed(() => Math.round(dspLoad.value * 100));
/** Warn once load sits at/over 80% for ~2 s (8 snapshots at 4 Hz). The EWMA
 *  underneath already smooths single-chunk spikes, so this is sustained. */
const hotStreak = ref(0);
watch(dspLoad, (v) => {
  hotStreak.value = showLoad.value && v >= 0.8 ? hotStreak.value + 1 : 0;
});
watch(showLoad, (v) => {
  if (!v) hotStreak.value = 0;
});
const loadWarning = computed(() => {
  if (hotStreak.value < 8) return null;
  if (dspLoad.value >= 1)
    return `DSP load over 100% (${loadPct.value}%) — glitches are happening. Try turning off a stage.`;
  return `DSP load high (${loadPct.value}%) — playback may glitch on this device. Try turning off a stage.`;
});
const showStages = ref(false);
const stageName = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);
</script>

<template>
  <section
    class="mb-5 rounded-lg border border-line bg-raised p-3 shadow-sm min-[720px]:sticky min-[720px]:top-0 min-[720px]:z-10"
    aria-label="Signal path"
    data-testid="signal-path"
  >
    <div class="grid grid-cols-[1fr_auto_1fr] gap-x-3 gap-y-1 text-sm">
      <!-- headers -->
      <h3 class="heading-3 m-0 flex flex-wrap items-center gap-1.5" data-testid="sp-file-title">
        Media file
        <UiBadge v-if="track?.mqa" variant="accent">{{ mqaLabel(track) }}</UiBadge>
      </h3>
      <span aria-hidden="true" />
      <h3 class="heading-3 m-0 flex flex-wrap items-center gap-1.5" data-testid="sp-out-title">
        Output device
        <UiBadge v-if="live?.exclusive" variant="ok" data-testid="sp-exclusive">Exclusive</UiBadge>
        <span class="truncate text-xs font-normal text-dim" data-testid="sp-device">
          {{ (live?.name ?? caps?.name ?? "").trim() }}{{ caps ? ` · ${caps.transport}` : "" }}
        </span>
      </h3>

      <!-- bit depth -->
      <div class="flex items-baseline justify-between gap-2" data-testid="sp-file-depth">
        <span class="text-dim">Bit depth</span><span class="tabular-nums">{{ fileDepth }}</span>
      </div>
      <span
        class="self-center text-center text-xs font-semibold"
        :class="linkTone[depth.state]"
        :title="depth.text"
        data-testid="sp-depth-link"
        :data-state="depth.state"
      >{{ linkMark[depth.state] }} <span class="font-normal">{{ depth.text }}</span></span>
      <div class="flex items-baseline justify-between gap-2" data-testid="sp-out-depth">
        <span class="text-dim">Bit depth</span><span class="tabular-nums">{{ outDepth }}</span>
      </div>

      <!-- sample rate -->
      <div class="flex items-baseline justify-between gap-2" data-testid="sp-file-rate">
        <span class="text-dim">Sample rate</span><span class="tabular-nums">{{ fileRate }}</span>
      </div>
      <span
        class="self-center text-center text-xs font-semibold"
        :class="linkTone[rate.state]"
        :title="rate.text"
        data-testid="sp-rate-link"
        :data-state="rate.state"
      >{{ linkMark[rate.state] }} <span class="font-normal">{{ rate.text }}</span></span>
      <div class="flex items-baseline justify-between gap-2" data-testid="sp-out-rate">
        <span class="text-dim">Sample rate</span><span class="tabular-nums">{{ outRate }}</span>
      </div>

      <!-- format / mode -->
      <div class="flex flex-col gap-0.5" data-testid="sp-file-format">
        <span class="text-dim">Format</span><span class="break-words text-[13px]">{{ fileFormat }}</span>
      </div>
      <span aria-hidden="true" />
      <div class="flex flex-col gap-0.5" data-testid="sp-out-mode">
        <span class="text-dim">Mode</span><span class="text-[13px]">{{ outMode }}</span>
      </div>

      <!-- DSP load -->
      <div v-if="showLoad" class="col-span-3 mt-1 border-t border-line pt-2" data-testid="sp-dsp-load">
        <div class="flex items-baseline justify-between gap-2">
          <span class="text-dim">DSP load</span>
          <span class="tabular-nums" data-testid="sp-dsp-pct">{{ loadPct }}%<span v-if="underruns > 0" class="text-faint"> · {{ underruns }} underrun{{ underruns === 1 ? "" : "s" }}</span></span>
        </div>
        <div
          class="mt-1 h-1 overflow-hidden rounded-full bg-active"
          role="meter"
          aria-label="DSP load"
          :aria-valuenow="loadPct"
          aria-valuemin="0"
          aria-valuemax="100"
        >
          <div
            class="h-full rounded-full transition-[width]"
            :class="dspLoad >= 1 ? 'bg-danger' : dspLoad >= 0.8 ? 'bg-warn' : 'bg-accent'"
            :style="{ width: `${Math.min(100, loadPct)}%` }"
          />
        </div>
        <p
          v-if="loadWarning"
          class="m-0 mt-1.5 text-xs"
          :class="dspLoad >= 1 ? 'text-danger-fg' : 'text-warn-fg'"
          data-testid="sp-dsp-warning"
        >
          {{ loadWarning }}
        </p>
        <button
          type="button"
          class="mt-1 border-0 bg-transparent p-0 text-xs text-accent hover:underline"
          :aria-expanded="showStages"
          data-testid="sp-dsp-stages-toggle"
          @click="showStages = !showStages"
        >
          {{ showStages ? "Hide stage load" : "Stage load" }}
        </button>
        <ul v-if="showStages" class="m-0 mt-1 list-none p-0" data-testid="sp-dsp-stages">
          <li
            v-for="[name, v] in dspStages"
            :key="name"
            class="flex items-center gap-2 py-0.5 text-xs"
            :class="v <= 0 && 'opacity-45'"
          >
            <span class="w-20 shrink-0 truncate text-dim">{{ stageName(name) }}</span>
            <span class="h-1 flex-1 overflow-hidden rounded-full bg-active">
              <span class="block h-full rounded-full bg-accent" :style="{ width: `${Math.min(100, Math.round(v * 100))}%` }" />
            </span>
            <span class="w-10 shrink-0 text-right tabular-nums text-faint">{{ Math.round(v * 100) }}%</span>
          </li>
        </ul>
        <p class="m-0 mt-1 text-[11px] text-faint">Measured on this device right now; heat and other apps move it.</p>
      </div>
    </div>
  </section>
</template>
