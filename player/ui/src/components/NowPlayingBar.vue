<script setup lang="ts">
import { ListMusic, Settings, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";
import { PopoverContent, PopoverPortal, PopoverRoot, PopoverTrigger } from "reka-ui";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import { useSettingsStore } from "../stores/settings";
import { STREAM_FORMATS, isPlayable, trackTitle, unplayableReason, type StreamFormat } from "../types";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import Artwork from "./Artwork.vue";
import SeekBar from "./SeekBar.vue";
import TrackFormatSelect from "./TrackFormatSelect.vue";
import TrackMenu from "./TrackMenu.vue";
import TransportControls from "./TransportControls.vue";
import VolumeSlider from "./VolumeSlider.vue";

const player = usePlayerStore();
const settings = useSettingsStore();
const lib = useLibraryStore();
const nav = useNavStore();

const track = computed(() => player.currentTrack);
const trackPlayable = computed(() => (track.value ? isPlayable(track.value) : false));

const formatTitle = computed(() =>
  !track.value
    ? "No track playing"
    : !trackPlayable.value
      ? unplayableReason(track.value)
      : "Stream format for this track",
);

const globalFormatOptions: UiSelectOption[] = [
  { value: null, label: "Auto (server default)" },
  ...STREAM_FORMATS.map((f) => ({ value: f, label: f === "passthrough" ? "Passthrough" : f.toUpperCase() })),
];

const artworkHash = computed(() => {
  const t = track.value;
  if (!t?.album_id) return null;
  return lib.albums.find((a) => a.id === t.album_id)?.artwork_hash ?? null;
});

function goNowPlaying(): void {
  if (track.value) nav.go("nowplaying");
}
</script>

<template>
  <footer class="relative z-20 border-t border-line bg-raised" data-testid="now-playing-bar">
    <div
      v-if="player.error"
      class="flex items-center gap-1.5 bg-danger/15 px-4 py-1.5 text-xs text-[#ff9d97]"
      role="alert"
      :title="player.error"
    >
      <TriangleAlert class="size-3.5 shrink-0" />
      <span class="truncate">{{ player.error }}</span>
    </div>
    <div class="grid min-h-[68px] grid-cols-[1fr_1.4fr_1fr] items-center gap-4 px-4 py-2">
      <!-- left: identity (click → full now-playing view) -->
      <button
        type="button"
        class="group flex min-w-0 items-center gap-2.5 border-0 bg-transparent p-0 text-left text-inherit"
        title="Open full now-playing view"
        data-testid="identity"
        @click="goNowPlaying"
      >
        <Artwork :hash="artworkHash" :size="44" :radius="6" :alt="track ? trackTitle(track) : 'No track'" />
        <div class="min-w-0">
          <div class="truncate font-semibold group-hover:text-accent" data-testid="title">
            {{ track ? trackTitle(track) : "Nothing playing" }}
          </div>
          <div class="truncate text-xs text-dim" data-testid="artist">{{ track?.artist ?? "—" }}</div>
        </div>
        <UiBadge v-if="player.chain" variant="accent" class="ml-1" :title="`Audio chain: ${player.chain}`">
          {{ player.chain }}
        </UiBadge>
        <UiBadge
          v-if="player.isDopExclusive"
          variant="ok"
          class="ml-1"
          title="Exclusive DoP output: bit-perfect, bypasses EQ, loudness, and volume"
        >
          Exclusive DoP
        </UiBadge>
      </button>

      <!-- center: transport + seek -->
      <div class="flex flex-col items-stretch gap-0.5">
        <TransportControls :disabled="!track" />
        <SeekBar :disabled="!track" />
      </div>

      <!-- right: format, volume, extras -->
      <div class="flex items-center justify-end gap-2.5">
        <TrackFormatSelect
          :track="track"
          :disabled="!trackPlayable"
          :title="formatTitle"
          trigger-class="w-[140px]"
        />
        <VolumeSlider class="w-[100px]" />
        <UiButton variant="icon" title="Queue" aria-label="Queue" @click="nav.go('queue')"><ListMusic /></UiButton>
        <TrackMenu v-if="track" :track="track" />
        <PopoverRoot>
          <PopoverTrigger as-child>
            <UiButton variant="icon" title="Playback settings" aria-label="Playback settings"><Settings /></UiButton>
          </PopoverTrigger>
          <PopoverPortal>
            <PopoverContent
              side="top"
              align="end"
              :side-offset="8"
              class="z-50 w-[230px] rounded-[10px] border border-line bg-hover p-3 shadow-[0_8px_24px_rgba(0,0,0,0.5)] outline-none"
            >
              <span class="mb-1.5 block text-xs text-dim">Default format</span>
              <UiSelect
                aria-label="Default format"
                trigger-class="w-full"
                :model-value="settings.globalFormat"
                :options="globalFormatOptions"
                @update:model-value="(v) => settings.saveGlobalFormat(v as StreamFormat | null)"
              />
              <p class="mb-0 mt-2 text-[11px] text-faint">Applies to tracks without a per-track override.</p>
            </PopoverContent>
          </PopoverPortal>
        </PopoverRoot>
      </div>
    </div>
  </footer>
</template>
