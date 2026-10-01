<script setup lang="ts">
import { Gauge } from "lucide-vue-next";
import { computed } from "vue";
import { connectionHealth, formatAhead, formatRate, gaugeFill } from "../lib/connection";
import { usePlayerStore } from "../stores/player";

/**
 * Connection gauge for the playback bar: how fast the server is delivering this
 * stream and how much audio is buffered ahead of the playhead. The bar is the
 * buffer (full at 30 s) and its colour is the health: green once there are
 * 10 s ahead, amber under that, red under 3 s. Hidden when nothing is streaming.
 */
const player = usePlayerStore();

const visible = computed(
  () =>
    player.currentTrack !== null &&
    player.status !== "stopped" &&
    (player.downloadBps !== null || player.bufferAheadMs !== null),
);
const health = computed(() => connectionHealth(player.bufferAheadMs, player.bufferComplete));
const fill = computed(() => gaugeFill(player.bufferAheadMs, player.bufferComplete));
const rate = computed(() => formatRate(player.downloadBps));
const ahead = computed(() => (player.bufferComplete ? "all" : formatAhead(player.bufferAheadMs)));

const label = computed(() => {
  const parts = [`Connection: ${rate.value === "—" ? "measuring" : rate.value}`];
  parts.push(player.bufferComplete ? "whole track buffered" : `${formatAhead(player.bufferAheadMs)} buffered ahead`);
  return parts.join(" · ");
});

const fillClass: Record<string, string> = {
  good: "bg-ok",
  fair: "bg-warn",
  low: "bg-danger",
  unknown: "bg-faint",
};
</script>

<template>
  <div
    v-if="visible"
    class="flex items-center gap-1.5 text-[11px] tabular-nums text-dim"
    role="img"
    :aria-label="label"
    :title="label"
    data-testid="connection-gauge"
    :data-health="health"
  >
    <Gauge class="size-3.5 shrink-0" aria-hidden="true" />
    <div class="flex flex-col gap-0.5 leading-none">
      <span data-testid="connection-rate">{{ rate }}</span>
      <span class="flex items-center gap-1">
        <span class="inline-block h-1 w-10 overflow-hidden rounded-full bg-line" aria-hidden="true">
          <span
            class="block h-full rounded-full transition-[width] duration-300"
            :class="fillClass[health]"
            :style="{ width: `${Math.round(fill * 100)}%` }"
            data-testid="connection-fill"
          />
        </span>
        <span data-testid="connection-ahead">{{ ahead }}</span>
      </span>
    </div>
  </div>
</template>
