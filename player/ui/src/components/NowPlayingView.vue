<script setup lang="ts">
import { ChevronLeft } from "lucide-vue-next";
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import { useAudiobooksStore } from "../stores/audiobooks";
import { audioFormat } from "../lib/audiobook";
import { audioPathLabel } from "../signalPath";
import { isPlayable, mqaLabel, mqaTitle, trackTitle, unplayableReason } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import TrackMenu from "./TrackMenu.vue";

const player = usePlayerStore();
const lib = useLibraryStore();
const nav = useNavStore();
const dsp = useDspStore();
const books = useAudiobooksStore();

const track = computed(() => player.currentTrack);

// --- composed audio-path badge (shared with the Settings signal-path panel) ---
const audioPath = computed(() => audioPathLabel(track.value, player, dsp));

const artworkHash = computed(() => {
  const t = track.value;
  if (!t?.album_id) return null;
  return lib.albums.find((a) => a.id === t.album_id)?.artwork_hash ?? null;
});

</script>

<template>
  <ViewShell width="medium">
    <UiButton variant="icon" class="mb-4" @click="nav.go('albums')"><ChevronLeft /> Library</UiButton>
    <StateMessage v-if="!track" kind="empty">
      <p class="m-0">Nothing playing.</p>
      <p class="m-0 mt-1 text-dim">Pick an album or playlist to start.</p>
    </StateMessage>
    <div v-else class="flex flex-col items-start gap-8 min-[720px]:flex-row">
      <div class="shrink-0">
        <Artwork :hash="artworkHash" :size="320" :radius="6" :alt="trackTitle(track)" />
      </div>
      <div class="min-w-0 flex-1">
        <h2 class="heading-1 m-0 mb-1" data-testid="np-title">{{ trackTitle(track) }}</h2>
        <p class="m-0 mb-0.5 text-base text-dim">{{ track.artist ?? "Unknown artist" }}</p>
        <p class="m-0 mb-4 text-sm text-faint">{{ track.album ?? "" }}</p>

        <div class="mb-5 flex flex-wrap gap-2">
          <UiBadge v-if="track.mqa" variant="accent" :title="mqaTitle(track)" data-testid="mqa-badge">{{ mqaLabel(track) }}</UiBadge>
          <UiBadge variant="accent" :title="`Audio chain: ${player.chain ?? '—'}`">{{ audioPath }}</UiBadge>
          <UiBadge v-if="books.isActive && audioFormat(track)" title="This file's format, sample rate, bitrate and channels" data-testid="np-book-format">{{ audioFormat(track) }}</UiBadge>
          <UiBadge
            v-if="player.isBitPerfect"
            variant="ok"
            data-testid="bit-perfect-badge"
            title="Bit-perfect: the file's samples go to the DAC untouched, at their own sample rate. EQ, loudness and volume are bypassed."
          >
            Bit-perfect
          </UiBadge>
          <UiBadge
            v-if="player.isDopExclusive"
            variant="ok"
            title="Exclusive DoP output: bit-perfect, bypasses EQ, loudness, and volume"
          >
            Exclusive DoP
          </UiBadge>
          <UiBadge v-if="!isPlayable(track)" variant="danger">{{ unplayableReason(track) }}</UiBadge>
        </div>

        <div class="mb-4"><TrackMenu :track="track" layout="buttons" :show-play-next="false" /></div>

        <StateMessage v-if="player.error" kind="error">{{ player.error }}</StateMessage>
      </div>
    </div>
  </ViewShell>
</template>
