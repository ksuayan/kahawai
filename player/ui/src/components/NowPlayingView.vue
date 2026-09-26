<script setup lang="ts">
import { ChevronLeft, SlidersHorizontal } from "lucide-vue-next";
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import { formatBadge, isPlayable, mqaLabel, mqaTitle, trackTitle, unplayableReason } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import SeekBar from "./SeekBar.vue";
import TrackFormatSelect from "./TrackFormatSelect.vue";
import TrackMenu from "./TrackMenu.vue";
import TransportControls from "./TransportControls.vue";
import VolumeSlider from "./VolumeSlider.vue";

const player = usePlayerStore();
const lib = useLibraryStore();
const nav = useNavStore();
const dsp = useDspStore();

const track = computed(() => player.currentTrack);

// --- composed audio-path badge ---
/** e.g. "DSF DSD64 → DoP → Exclusive (bit-perfect)" or
 *  "FLAC 44.1k/16 → FLAC transcode → PCM shared · EQ on". */
const audioPath = computed(() => {
  const t = track.value;
  if (!t) return "Nothing playing";
  const src = formatBadge(t);
  const stream = player.activeFormat ? player.activeFormat.toUpperCase() : "AUTO";
  const out = player.isDopExclusive
    ? "Exclusive DoP · bit-perfect"
    : player.isBitPerfect
      ? "Bit-perfect · exclusive"
      : "PCM shared";
  const dspBits: string[] = [];
  if (!player.isExclusive) {
    if (dsp.eqEnabled && dsp.activeBands.length > 0) dspBits.push(`EQ ${dsp.activeBands.length} bands`);
    if (dsp.loudnessEnabled) dspBits.push(`Loudness ${dsp.loudnessTarget} LUFS`);
  }
  return `${src} → ${stream} → ${out}${dspBits.length ? ` · ${dspBits.join(" · ")}` : ""}`;
});

const artworkHash = computed(() => {
  const t = track.value;
  if (!t?.album_id) return null;
  return lib.albums.find((a) => a.id === t.album_id)?.artwork_hash ?? null;
});

function goEq(): void {
  nav.go("settings");
}

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
        <Artwork :hash="artworkHash" :size="320" :radius="12" :alt="trackTitle(track)" />
      </div>
      <div class="min-w-0 flex-1">
        <h2 class="m-0 mb-1 text-2xl font-semibold" data-testid="np-title">{{ trackTitle(track) }}</h2>
        <p class="m-0 mb-0.5 text-base text-dim">{{ track.artist ?? "Unknown artist" }}</p>
        <p class="m-0 mb-4 text-sm text-faint">{{ track.album ?? "" }}</p>

        <div class="mb-5 flex flex-wrap gap-2">
          <UiBadge>{{ formatBadge(track) }}</UiBadge>
          <UiBadge v-if="track.mqa" variant="accent" :title="mqaTitle(track)" data-testid="mqa-badge">{{ mqaLabel(track) }}</UiBadge>
          <UiBadge variant="accent" :title="`Audio chain: ${player.chain ?? '—'}`">{{ audioPath }}</UiBadge>
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

        <SeekBar size="md" />
        <p class="mb-5 mt-1 text-[11px] text-faint">
          Buffer: not exposed by the engine — the stream is progressive HTTP (the position above is the playhead).
        </p>

        <div class="mb-5"><TransportControls large :show-stop="false" /></div>

        <div class="mb-4 flex flex-wrap items-center gap-4">
          <label class="flex items-center gap-2 text-xs text-dim">
            Volume
            <VolumeSlider class="w-[120px]" />
          </label>
          <div class="flex items-center gap-2 text-xs text-dim">
            Stream this track as
            <TrackFormatSelect :track="track" :disabled="!isPlayable(track)" />
          </div>
          <UiButton variant="icon" title="Open EQ in Settings" @click="goEq"><SlidersHorizontal /> EQ</UiButton>
          <TrackMenu :track="track" />
        </div>

        <StateMessage v-if="player.error" kind="error">{{ player.error }}</StateMessage>
      </div>
    </div>
  </ViewShell>
</template>
