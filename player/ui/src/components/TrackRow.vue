<script setup lang="ts">
import { computed } from "vue";
import {
  formatBadge,
  formatDuration,
  isPlayable,
  mqaLabel,
  mqaTitle,
  trackTitle,
  unplayableReason,
  type Track,
} from "../types";
import UiBadge from "../ui/UiBadge.vue";
import Artwork from "./Artwork.vue";
import TrackMenu from "./TrackMenu.vue";

const props = withDefaults(
  defineProps<{
    track: Track;
    number?: number | null;
    showArtwork?: boolean;
    artworkHash?: string | null;
    current?: boolean;
    /** Show the ⋯ action menu (play next / add to queue / playlist / ISO). */
    showMenu?: boolean;
  }>(),
  { showArtwork: false, current: false, showMenu: true },
);

const emit = defineEmits<{
  (e: "play", track: Track): void;
}>();

const playable = computed(() => isPlayable(props.track));
const title = computed(() => trackTitle(props.track));
const reason = computed(() => unplayableReason(props.track));

function onDblClick(): void {
  if (playable.value) emit("play", props.track);
}
</script>

<template>
  <div
    class="flex cursor-default items-center gap-3 rounded-md px-2.5 py-[7px]"
    :class="[current ? 'bg-accent/15' : 'hover:bg-hover', !playable && 'opacity-45']"
    :data-current="current || undefined"
    :data-playable="playable"
    :title="playable ? title : `${title} — ${reason}`"
    @dblclick="onDblClick"
  >
    <span class="w-7 shrink-0 text-right tabular-nums text-faint">{{ number ?? track.track_no ?? "–" }}</span>
    <Artwork v-if="showArtwork" :hash="artworkHash" :size="36" :radius="4" />
    <div class="min-w-0 flex-1">
      <div class="truncate">{{ title }}</div>
      <div v-if="track.artist || track.album" class="truncate text-xs text-dim">
        {{ [track.artist, track.album].filter(Boolean).join(" — ") }}
      </div>
    </div>
    <UiBadge>{{ formatBadge(track) }}</UiBadge>
    <UiBadge v-if="track.mqa" variant="accent" :title="mqaTitle(track)" data-testid="mqa-badge">{{ mqaLabel(track) }}</UiBadge>
    <span class="shrink-0 tabular-nums text-dim">{{ formatDuration(track.duration_ms) }}</span>
    <TrackMenu v-if="showMenu" :track="track" />
    <slot />
  </div>
</template>
