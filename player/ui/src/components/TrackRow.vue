<script setup lang="ts">
import { computed, ref } from "vue";
import { fireContextMenu, useLongPress } from "../lib/longpress";
import { joinParts } from "../lib/format";
import { useBreakpoint } from "../lib/breakpoint";
import {
  formatBadge,
  formatDuration,
  isPlayable,
  mqaLabel,
  mqaTitle,
  qualityTitle,
  trackTitle,
  unplayableReason,
  type Track,
} from "../types";
import UiBadge from "../ui/UiBadge.vue";
import Artwork from "./Artwork.vue";
import ItemContextMenu from "./ItemContextMenu.vue";
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

const { isPhone } = useBreakpoint();
const rowEl = ref<HTMLElement | null>(null);
// Long-press opens the same context menu as right-click.
useLongPress(rowEl, (e) => fireContextMenu(e));

/** Phones have no double-click affordance: a single tap plays. Taps on inner
 *  controls (the ⋯ menu, badges) are left alone. */
function onTap(e: MouseEvent): void {
  if (!isPhone.value || !playable.value) return;
  if ((e.target as HTMLElement).closest("button, a, [role='button']")) return;
  emit("play", props.track);
}
</script>

<template>
  <ItemContextMenu :track="track" @play="emit('play', track)">
  <div
    ref="rowEl"
    class="flex cursor-default items-center gap-3 rounded-md px-2.5 py-[7px]"
    :class="[
      current ? 'bg-accent/15' : 'hover:bg-hover',
      !playable && 'opacity-45',
      isPhone && 'min-h-[52px] py-3',
    ]"
    :data-current="current || undefined"
    :data-playable="playable"
    :title="playable ? title : `${title} — ${reason}`"
    @dblclick="onDblClick"
    @click="onTap($event)"
  >
    <span class="w-7 shrink-0 text-right tabular-nums text-faint">{{ number ?? track.track_no ?? "–" }}</span>
    <Artwork v-if="showArtwork" :hash="artworkHash" :size="36" :radius="4" />
    <div class="min-w-0 flex-1">
      <div class="truncate">{{ title }}</div>
      <div v-if="track.artist || track.album" class="truncate text-xs text-dim">
        {{ joinParts([track.artist, track.album], " — ") }}
      </div>
    </div>
    <UiBadge class="max-[719px]:hidden" :title="qualityTitle(track)" data-testid="format-badge">{{ formatBadge(track) }}</UiBadge>
    <UiBadge v-if="track.mqa" class="max-[719px]:hidden" variant="accent" :title="mqaTitle(track)" data-testid="mqa-badge">{{ mqaLabel(track) }}</UiBadge>
    <span class="shrink-0 tabular-nums text-dim">{{ formatDuration(track.duration_ms) }}</span>
    <TrackMenu v-if="showMenu" :track="track" />
    <slot />
  </div>
  </ItemContextMenu>
</template>
