<script setup lang="ts">
import { computed } from "vue";
import { isPlayable, trackTitle, unplayableReason, type Track } from "../types";
import Artwork from "./Artwork.vue";
import ItemContextMenu from "./ItemContextMenu.vue";
import TrackMenu from "./TrackMenu.vue";

/** One track in a grid: its album's cover, title, artist. Click plays it. */
const props = withDefaults(
  defineProps<{
    track: Track;
    artworkHash?: string | null;
    current?: boolean;
    /** Small label on the cover (a queue position). */
    badge?: string | number | null;
    showMenu?: boolean;
  }>(),
  { artworkHash: null, current: false, badge: null, showMenu: true },
);
const emit = defineEmits<{ (e: "play", track: Track): void }>();

const playable = computed(() => isPlayable(props.track));
const title = computed(() => trackTitle(props.track));
</script>

<template>
  <ItemContextMenu :track="track" @play="emit('play', track)">
  <div
    class="group relative"
    :class="!playable && 'opacity-45'"
    :data-current="current || undefined"
    :data-playable="playable"
    data-testid="track-card"
  >
    <button
      type="button"
      class="block w-full rounded-lg p-0 text-left outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
      :title="playable ? `${title}\nClick to play` : `${title} — ${unplayableReason(track)}`"
      :disabled="!playable"
      @click="emit('play', track)"
    >
      <div class="relative">
        <Artwork
          :hash="artworkHash"
          :radius="6"
          :alt="track.album ?? title"
          fluid
          class="transition group-hover:brightness-110"
          :class="current && 'outline outline-2 -outline-offset-2 outline-accent'"
        />
        <span
          v-if="badge !== null"
          class="absolute left-1.5 top-1.5 rounded bg-canvas/80 px-1.5 py-px text-[11px] tabular-nums text-dim"
        >
          {{ badge }}
        </span>
      </div>
      <div class="mt-2 truncate font-semibold" :class="current && 'text-accent'">{{ title }}</div>
      <div class="truncate text-xs text-dim">{{ [track.artist, track.album].filter(Boolean).join(" — ") }}</div>
    </button>
    <div v-if="showMenu" class="absolute right-1.5 top-1.5 opacity-0 transition group-hover:opacity-100 focus-within:opacity-100">
      <TrackMenu :track="track" />
    </div>
  </div>
  </ItemContextMenu>
</template>
