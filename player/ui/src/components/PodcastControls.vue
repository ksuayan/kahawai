<script setup lang="ts">
import { ListEnd, Music, RotateCcw, RotateCw } from "lucide-vue-next";
import { SPEEDS, speedLabel } from "../lib/audiobook";
import { useNavStore } from "../stores/nav";
import { usePodcastsStore } from "../stores/podcasts";
import UiButton from "../ui/UiButton.vue";
import UiSelect from "../ui/UiSelect.vue";

/** The podcast row of the playback bar: skips (the show's own seconds), speed, Up Next, and the way back to the music. */
const podcasts = usePodcastsStore();
const nav = useNavStore();
const speedOptions = SPEEDS.map((s) => ({ value: String(s), label: speedLabel(s) }));
</script>

<template>
  <div class="flex flex-wrap items-center justify-center gap-1.5" data-testid="podcast-controls">
    <UiButton variant="icon" :title="`Back ${podcasts.skipBackS} s (J)`" :aria-label="`Back ${podcasts.skipBackS} seconds`" data-testid="podcast-skip-back" @click="podcasts.skip(-1)">
      <RotateCcw /><span class="text-[11px] tabular-nums">{{ podcasts.skipBackS }}</span>
    </UiButton>
    <UiButton variant="icon" :title="`Forward ${podcasts.skipForwardS} s (L)`" :aria-label="`Forward ${podcasts.skipForwardS} seconds`" data-testid="podcast-skip-forward" @click="podcasts.skip(1)">
      <RotateCw /><span class="text-[11px] tabular-nums">{{ podcasts.skipForwardS }}</span>
    </UiButton>
    <UiSelect
      aria-label="Playback speed"
      trigger-class="w-[84px]"
      :model-value="String(podcasts.speed)"
      :options="speedOptions"
      title="Playback speed for this show (the pitch is kept)"
      @update:model-value="(v) => podcasts.setSpeed(Number(v))"
    />
    <UiButton variant="icon" :title="`Up Next (${podcasts.upNext.length})`" aria-label="Up Next" data-testid="podcast-up-next" @click="nav.go('podcasts')">
      <ListEnd /><span v-if="podcasts.upNext.length" class="text-[11px] tabular-nums">{{ podcasts.upNext.length }}</span>
    </UiButton>
    <UiButton v-if="podcasts.stash" title="Put the episode down and go back to the music" data-testid="podcast-back-to-music" @click="podcasts.returnToMusic()">
      <Music /> Back to music
    </UiButton>
  </div>
</template>
