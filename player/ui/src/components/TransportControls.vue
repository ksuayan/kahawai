<script setup lang="ts">
import { LoaderCircle, Pause, Play, Repeat, Repeat1, Shuffle, SkipBack, SkipForward, Square } from "lucide-vue-next";
import { usePlayerStore } from "../stores/player";
import UiButton from "../ui/UiButton.vue";

/** Shuffle · previous · play/pause · next · (stop) · repeat. */
const props = withDefaults(defineProps<{ large?: boolean; showStop?: boolean; disabled?: boolean }>(), {
  large: false,
  showStop: true,
  disabled: false,
});
const player = usePlayerStore();

function repeatTitle(): string {
  return player.repeat === "off" ? "Repeat off" : player.repeat === "all" ? "Repeat all" : "Repeat one";
}
const side = () => (props.large ? "lg" : "md");
</script>

<template>
  <div class="flex items-center" :class="large ? 'gap-2' : 'justify-center gap-1'">
    <UiButton variant="icon" :size="side()" :pressed="player.shuffle" title="Shuffle" aria-label="Shuffle" :disabled="disabled" @click="player.toggleShuffle()"><Shuffle /></UiButton>
    <UiButton variant="icon" :size="side()" title="Previous (P)" aria-label="Previous" :disabled="disabled" @click="player.prevTrack()"><SkipBack class="fill-current" /></UiButton>
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
    <UiButton variant="icon" :size="side()" title="Next (N)" aria-label="Next" :disabled="disabled" @click="player.nextTrack()"><SkipForward class="fill-current" /></UiButton>
    <UiButton v-if="showStop" variant="icon" :size="side()" title="Stop" aria-label="Stop" :disabled="disabled" @click="player.stop()"><Square class="fill-current" /></UiButton>
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
  </div>
</template>
