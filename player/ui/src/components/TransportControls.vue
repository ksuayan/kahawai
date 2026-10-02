<script setup lang="ts">
import { LoaderCircle, Pause, Play, Repeat, Repeat1, Shuffle, SkipBack, SkipForward } from "lucide-vue-next";
import { useAudiobooksStore } from "../stores/audiobooks";
import { usePlayerStore } from "../stores/player";
import UiButton from "../ui/UiButton.vue";
import EqControl from "./EqControl.vue";

/** Previous · play/pause · next, with shuffle and repeat grouped apart on the right. */
const props = withDefaults(defineProps<{ large?: boolean; disabled?: boolean }>(), {
  large: false,
  disabled: false,
});
const player = usePlayerStore();
const books = useAudiobooksStore();

function repeatTitle(): string {
  return player.repeat === "off" ? "Repeat off" : player.repeat === "all" ? "Repeat all" : "Repeat one";
}
const side = () => (props.large ? "lg" : "md");
</script>

<template>
  <div class="grid grid-cols-[1fr_auto_1fr] items-center gap-2">
    <span />
    <div class="flex items-center" :class="large ? 'gap-2' : 'gap-1'" data-testid="main-controls">
      <UiButton variant="icon" :size="side()" title="Previous (P)" aria-label="Previous" :disabled="disabled" @click="books.isActive ? books.previousChapter() : player.prevTrack()"><SkipBack class="fill-current" /></UiButton>
      <UiButton
      variant="icon-strong"
      :size="large ? 'xl' : 'lg'"
      :title="player.isPlaying ? 'Pause (Space)' : 'Play (Space)'"
      :aria-label="player.isPlaying ? 'Pause' : 'Play'"
      :disabled="disabled"
      data-testid="play-pause"
      @click="player.toggle()"
    >
      <LoaderCircle v-if="player.isLoading" class="animate-spin" />
      <Pause v-else-if="player.isPlaying" class="fill-current" />
      <Play v-else class="fill-current" />
    </UiButton>
      <UiButton variant="icon" :size="side()" title="Next (N)" aria-label="Next" :disabled="disabled" @click="books.isActive ? books.nextChapter() : player.nextTrack()"><SkipForward class="fill-current" /></UiButton>
    </div>
    <div class="flex items-center justify-end gap-1" data-testid="mode-controls">
      <UiButton variant="icon" :size="side()" :pressed="player.shuffle" title="Shuffle" aria-label="Shuffle" :disabled="disabled" @click="player.toggleShuffle()"><Shuffle /></UiButton>
      <UiButton
        variant="icon"
        :size="side()"
        :pressed="player.repeat !== 'off'"
        :title="repeatTitle()"
        :aria-label="repeatTitle()"
        :disabled="disabled"
        @click="player.cycleRepeat()"
      >
        <Repeat1 v-if="player.repeat === 'one'" />
        <Repeat v-else />
      </UiButton>
      <EqControl :size="side()" :disabled="disabled" />
    </div>
  </div>
</template>
