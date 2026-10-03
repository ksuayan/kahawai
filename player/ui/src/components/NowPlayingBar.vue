<script setup lang="ts">
import { Info, ListOrdered, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";
import { useAudiobooksStore } from "../stores/audiobooks";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import { useRadioStore } from "../stores/radio";
import { usePodcastsStore } from "../stores/podcasts";
import { trackTitle } from "../types";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";
import Artwork from "./Artwork.vue";
import AudiobookControls from "./AudiobookControls.vue";
import PodcastControls from "./PodcastControls.vue";
import ConnectionGauge from "./ConnectionGauge.vue";
import SeekBar from "./SeekBar.vue";
import TrackMenu from "./TrackMenu.vue";
import TransportControls from "./TransportControls.vue";
import VolumeSlider from "./VolumeSlider.vue";

const player = usePlayerStore();
const lib = useLibraryStore();
const nav = useNavStore();
const books = useAudiobooksStore();
const radio = useRadioStore();
const podcasts = usePodcastsStore();
/** The episode playing, when it is one. */
const episode = computed(() => (podcasts.isActive ? podcasts.active : null));

const track = computed(() => player.currentTrack);
const artworkHash = computed(() => {
  if (books.isActive) return books.active?.cover_hash ?? null;
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
      class="flex items-center gap-1.5 bg-danger/15 px-4 py-1.5 text-xs text-danger-fg"
      role="alert"
      :title="player.error"
    >
      <TriangleAlert class="size-3.5 shrink-0" />
      <span class="truncate">{{ player.error }}</span>
    </div>
    <div
      v-else-if="radio.now?.reconnecting"
      class="flex items-center gap-1.5 bg-accent/10 px-4 py-1.5 text-xs text-dim"
      role="status"
      data-testid="radio-reconnecting"
    >
      <Info class="size-3.5 shrink-0" />
      <span class="truncate">
        Lost the connection to {{ radio.stationName }}. Trying again… (try {{ radio.now.attempt }})
      </span>
    </div>
    <div
      v-else-if="player.notice"
      class="flex items-center gap-1.5 bg-accent/10 px-4 py-1.5 text-xs text-dim"
      role="status"
      data-testid="playback-notice"
      :title="player.notice"
    >
      <Info class="size-3.5 shrink-0" />
      <span class="truncate">{{ player.notice }}</span>
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
        <Artwork
          :hash="artworkHash"
          :url="episode ? episode.episode.image_url || episode.feed.image_url : null"
          :placeholder="books.isActive ? 'book' : episode ? 'podcast' : radio.isPlaying ? 'radio' : 'music'"
          :size="44"
          :radius="6"
          :alt="track ? trackTitle(track) : 'No track'"
        />
        <div class="min-w-0">
          <div class="truncate font-semibold group-hover:text-accent" data-testid="title">
            {{ books.isActive && books.active ? books.active.title : track ? trackTitle(track) : "Nothing playing" }}
          </div>
          <div class="truncate text-xs text-dim" data-testid="artist">
            <template v-if="books.isActive">{{ books.chapter?.title ?? books.active?.title }} · {{ books.active?.author ?? "" }}</template>
            <template v-else-if="episode">{{ episode.feed.title }}</template>
            <template v-else-if="radio.isPlaying">{{ radio.now?.title ?? "Live radio" }}</template>
            <template v-else>{{ track?.artist ?? "—" }}</template>
          </div>
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
        <UiBadge
          v-if="player.isBitPerfect"
          variant="ok"
          class="ml-1"
          data-testid="bit-perfect-badge"
          title="Bit-perfect: the file's samples go to the DAC untouched, at their own sample rate. EQ, loudness and volume are bypassed."
        >
          Bit-perfect
        </UiBadge>
      </button>

      <!-- center: transport + seek -->
      <div class="flex flex-col items-stretch gap-0.5">
        <AudiobookControls v-if="books.isActive" />
        <PodcastControls v-else-if="episode" />
        <TransportControls :disabled="!track" />
        <SeekBar :disabled="!track" />
      </div>

      <!-- right: volume, extras -->
      <div class="flex items-center justify-end gap-2.5">
        <ConnectionGauge />
        <VolumeSlider class="w-[100px]" />
        <UiButton variant="icon" title="Queue" aria-label="Queue" @click="nav.go('queue')"><ListOrdered /></UiButton>
        <TrackMenu v-if="track" :track="track" />
      </div>
    </div>
  </footer>
</template>
