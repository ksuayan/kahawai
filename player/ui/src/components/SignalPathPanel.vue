<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, watch } from "vue";
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
</script>

<template>
  <section
    class="sticky top-0 z-10 mb-5 rounded-lg border border-line bg-raised p-3 shadow-sm"
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
    </div>
  </section>
</template>
