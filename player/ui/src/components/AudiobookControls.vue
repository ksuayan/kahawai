<script setup lang="ts">
import { BookmarkPlus, Check, Mic, Moon, Music, RotateCcw, RotateCw } from "lucide-vue-next";
import { computed } from "vue";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "reka-ui";
import { clock, SLEEP_MINUTES, SPEEDS, speedLabel } from "../lib/audiobook";
import { useAudiobooksStore } from "../stores/audiobooks";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import UiButton from "../ui/UiButton.vue";
import UiSelect from "../ui/UiSelect.vue";

/**
 * The audiobook row of the playback bar: speed, skip back and forward (the
 * book's own seconds), a bookmark at the playhead, the sleep timer, the
 * voice settings, and the way back to the music.
 */
const books = useAudiobooksStore();
const dsp = useDspStore();
const player = usePlayerStore();

const speedOptions = SPEEDS.map((s) => ({ value: String(s), label: speedLabel(s) }));
const sleepText = computed(() => {
  const left = books.sleepRemainingMs;
  if (left === null) return "";
  return books.sleep?.kind === "chapter" ? `chapter · ${clock(left)}` : clock(left);
});
const itemClass =
  "flex cursor-default select-none items-center justify-between gap-3 whitespace-nowrap rounded-md px-2.5 py-2 text-[13px] text-fg outline-none " +
  "data-[disabled]:opacity-40 data-[highlighted]:bg-hover";
const contentClass = "z-[60] min-w-[180px] rounded-md border border-line bg-surface p-1 shadow-float";
</script>

<template>
  <div class="flex flex-wrap items-center justify-center gap-1.5" data-testid="audiobook-controls">
    <UiButton variant="icon" :title="`Back ${books.skipBackS} s (J)`" :aria-label="`Back ${books.skipBackS} seconds`" data-testid="skip-back" @click="books.skip(-1)">
      <RotateCcw /><span class="text-[11px] tabular-nums">{{ books.skipBackS }}</span>
    </UiButton>
    <UiButton variant="icon" :title="`Forward ${books.skipForwardS} s (L)`" :aria-label="`Forward ${books.skipForwardS} seconds`" data-testid="skip-forward" @click="books.skip(1)">
      <RotateCw /><span class="text-[11px] tabular-nums">{{ books.skipForwardS }}</span>
    </UiButton>
    <UiSelect
      aria-label="Playback speed"
      trigger-class="w-[84px]"
      :model-value="String(books.speed)"
      :options="speedOptions"
      title="Playback speed (the pitch is kept)"
      @update:model-value="(v) => books.setSpeed(Number(v))"
    />
    <UiButton variant="icon" title="Add a bookmark here" aria-label="Add a bookmark" data-testid="bookmark-here" @click="books.addBookmark()">
      <BookmarkPlus />
    </UiButton>

    <DropdownMenuRoot>
      <DropdownMenuTrigger as-child>
        <UiButton variant="icon" :pressed="books.sleep !== null" title="Sleep timer" aria-label="Sleep timer" data-testid="sleep-button">
          <Moon /><span v-if="sleepText" class="text-[11px] tabular-nums" data-testid="sleep-left">{{ sleepText }}</span>
        </UiButton>
      </DropdownMenuTrigger>
      <DropdownMenuPortal>
        <DropdownMenuContent align="center" :side-offset="6" side="top" :class="contentClass" data-kw-fade>
          <DropdownMenuItem v-for="m in SLEEP_MINUTES" :key="m" :class="itemClass" :data-testid="`sleep-${m}`" @select="books.startSleepMinutes(m)">
            {{ m }} minutes
          </DropdownMenuItem>
          <DropdownMenuItem :class="itemClass" data-testid="sleep-chapter" @select="books.startSleepEndOfChapter()">End of chapter</DropdownMenuItem>
          <template v-if="books.sleep">
            <DropdownMenuSeparator class="my-1 h-px bg-line" />
            <DropdownMenuItem :class="itemClass" data-testid="sleep-cancel" @select="books.cancelSleep(true)">Cancel timer</DropdownMenuItem>
          </template>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>

    <UiButton
      variant="icon"
      :pressed="dsp.voicePresetOn"
      :disabled="player.isExclusive"
      title="Voice settings: Spoken word EQ, loudness and limiter"
      aria-label="Voice settings"
      data-testid="voice-preset"
      @click="dsp.applyVoicePreset()"
    >
      <Mic /><Check v-if="dsp.voicePresetOn" class="!size-3" />
    </UiButton>
    <UiButton v-if="books.stash" title="Put the book down and go back to the music" data-testid="back-to-music" @click="books.returnToMusic()">
      <Music /> Back to music
    </UiButton>
  </div>
</template>
