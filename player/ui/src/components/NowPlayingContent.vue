<script setup lang="ts">
withDefaults(defineProps<{ compact?: boolean }>(), { compact: false });
import { BookOpen, BookmarkPlus, Info, ListEnd, Moon, Music, Podcast } from "lucide-vue-next";
import { computed } from "vue";
import { useDspStore } from "../stores/dsp";
import { useLibraryStore } from "../stores/library";
import { usePlayerStore } from "../stores/player";
import { useAudiobooksStore } from "../stores/audiobooks";
import { useRadioStore } from "../stores/radio";
import { usePodcastsStore } from "../stores/podcasts";
import { episodeDate } from "../lib/podcast";
import { audioFormat, clock, duration, remainingText, speedLabel } from "../lib/audiobook";
import { useNavStore } from "../stores/nav";
import { audioPathLabel } from "../signalPath";
import { isPlayable, mqaLabel, mqaTitle, trackTitle, unplayableReason } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";
import Artwork from "./Artwork.vue";
import TrackMenu from "./TrackMenu.vue";

const player = usePlayerStore();
const lib = useLibraryStore();
const dsp = useDspStore();
const books = useAudiobooksStore();
const radio = useRadioStore();
const podcasts = usePodcastsStore();
/** The episode playing, when it is one: the screen shows the episode and its show. */
const episode = computed(() => (podcasts.isActive ? podcasts.active : null));
const episodeProgress = computed(() => {
  const d = episode.value?.episode.duration_ms ?? player.durationMs ?? 0;
  return d > 0 ? Math.min(1, Math.max(0, player.positionMs / d)) : 0;
});
const nav = useNavStore();

/** The book playing, when it is a book: the screen shows the book, not the file. */
const book = computed(() => (books.isActive ? books.active : null));
const byline = computed(() => {
  const b = book.value;
  if (!b) return "";
  return [b.author && `by ${b.author}`, b.narrator && `read by ${b.narrator}`].filter(Boolean).join(" · ");
});
const series = computed(() => {
  const b = book.value;
  if (!b?.series) return "";
  return b.series_index != null ? `${b.series}, book ${b.series_index}` : b.series;
});
/** "Chapter 3 of 12", and how far into it: "12:03 of 45:00". */
const chapterLine = computed(() => {
  const b = book.value;
  if (!b || books.chapterIndex < 0 || b.chapters.length < 2) return null;
  const c = books.chapter!;
  return {
    position: `Chapter ${books.chapterIndex + 1} of ${b.chapters.length}`,
    title: c.title,
    time: `${clock(Math.max(0, books.offsetMs - c.start_offset_ms))} of ${clock(c.duration_ms)}`,
  };
});
const bookProgress = computed(() => {
  const b = book.value;
  if (!b || b.duration_ms <= 0) return 0;
  return Math.min(1, Math.max(0, books.offsetMs / b.duration_ms));
});

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
<StateMessage v-if="!track" kind="empty">
  <p class="m-0">Nothing playing.</p>
  <p class="m-0 mt-1 text-dim">Pick an album or playlist to start.</p>
</StateMessage>
<div v-else-if="book" :class="compact ? 'flex flex-col items-start gap-4' : 'flex flex-col items-start gap-8 min-[720px]:flex-row'" data-testid="np-book">
  <div class="shrink-0">
    <Artwork :hash="book.cover_hash" placeholder="book" :size="compact ? 160 : 320" :radius="6" :alt="book.title" />
  </div>
  <div class="min-w-0 flex-1">
    <p class="m-0 mb-2 flex items-center gap-1.5 text-xs uppercase tracking-wide text-faint" data-testid="np-kind">
      <BookOpen class="size-3.5" aria-hidden="true" /> Audiobook
    </p>
    <h2 class="heading-1 m-0 mb-1" data-testid="np-title">{{ book.title }}</h2>
    <p v-if="byline" class="m-0 mb-0.5 text-base text-dim" data-testid="np-byline">{{ byline }}</p>
    <p v-if="series" class="m-0 text-sm text-faint" data-testid="np-series">{{ series }}</p>

    <div v-if="chapterLine" class="mt-5" data-testid="np-chapter">
      <p class="m-0 text-xs text-faint">{{ chapterLine.position }}</p>
      <p class="m-0 truncate font-semibold">{{ chapterLine.title }}</p>
      <p class="m-0 text-xs tabular-nums text-dim">{{ chapterLine.time }}</p>
    </div>

    <div class="mt-5 max-w-[420px]" data-testid="np-book-progress">
      <div class="h-1.5 overflow-hidden rounded-full bg-active">
        <div class="h-full bg-accent" :style="{ width: `${Math.round(bookProgress * 100)}%` }" />
      </div>
      <p class="m-0 mt-1.5 flex justify-between gap-3 text-xs tabular-nums text-dim">
        <span>{{ clock(books.offsetMs) }} of {{ duration(book.duration_ms) }} · {{ Math.round(bookProgress * 100) }}%</span>
        <span>{{ remainingText(book.duration_ms, books.offsetMs, books.speed) }}</span>
      </p>
    </div>

    <div class="mb-5 mt-5 flex flex-wrap gap-2">
      <UiBadge title="Playback speed (the pitch is kept)" data-testid="np-speed">{{ speedLabel(books.speed) }}</UiBadge>
      <UiBadge v-if="books.sleepRemainingMs !== null" title="Sleep timer" data-testid="np-sleep"><Moon class="size-3" /> {{ clock(books.sleepRemainingMs) }}</UiBadge>
      <UiBadge v-if="audioFormat(track)" title="This file's format, sample rate, bitrate and channels" data-testid="np-book-format">{{ audioFormat(track) }}</UiBadge>
      <UiBadge variant="accent" :title="`Audio chain: ${player.chain ?? '—'}`">{{ audioPath }}</UiBadge>
    </div>

    <div class="mb-4 flex flex-wrap items-center gap-2">
      <UiButton data-testid="np-bookmark" @click="books.addBookmark()"><BookmarkPlus /> Add bookmark at {{ clock(books.offsetMs) }}</UiButton>
      <UiButton data-testid="np-book-details" @click="nav.go('audiobook', book.id)"><Info /> Book details</UiButton>
      <UiButton v-if="books.stash" data-testid="np-back-to-music" @click="books.returnToMusic()"><Music /> Back to music</UiButton>
    </div>

    <StateMessage v-if="player.error" kind="error">{{ player.error }}</StateMessage>
  </div>
</div>
<div v-else-if="episode" :class="compact ? 'flex flex-col items-start gap-4' : 'flex flex-col items-start gap-8 min-[720px]:flex-row'" data-testid="np-podcast">
  <div class="shrink-0">
    <Artwork :url="episode.episode.image_url || episode.feed.image_url" placeholder="podcast" :size="compact ? 160 : 320" :radius="6" :alt="episode.feed.title" />
  </div>
  <div class="min-w-0 flex-1">
    <p class="m-0 mb-2 flex items-center gap-1.5 text-xs uppercase tracking-wide text-faint" data-testid="np-kind">
      <Podcast class="size-3.5" aria-hidden="true" /> Podcast
    </p>
    <h2 class="heading-1 m-0 mb-1" data-testid="np-title">{{ episode.episode.title }}</h2>
    <p class="m-0 mb-0.5 text-base text-dim" data-testid="np-show">{{ episode.feed.title }}</p>
    <p v-if="episode.episode.published_at" class="m-0 text-sm text-faint">{{ episodeDate(episode.episode.published_at) }}</p>

    <div class="mt-5 max-w-[420px]" data-testid="np-episode-progress">
      <div class="h-1.5 overflow-hidden rounded-full bg-active">
        <div class="h-full bg-accent" :style="{ width: `${Math.round(episodeProgress * 100)}%` }" />
      </div>
      <p v-if="episode.episode.duration_ms" class="m-0 mt-1.5 flex justify-between gap-3 text-xs tabular-nums text-dim">
        <span>{{ clock(player.positionMs) }} of {{ duration(episode.episode.duration_ms) }}</span>
        <span>{{ remainingText(episode.episode.duration_ms, player.positionMs, podcasts.speed) }}</span>
      </p>
    </div>

    <div class="mb-5 mt-5 flex flex-wrap gap-2">
      <UiBadge title="Playback speed for this show (the pitch is kept)" data-testid="np-speed">{{ speedLabel(podcasts.speed) }}</UiBadge>
      <UiBadge v-if="audioFormat(track)" title="This file's format" data-testid="np-episode-format">{{ audioFormat(track) }}</UiBadge>
      <UiBadge :title="episode.episode.downloaded ? 'Playing the file downloaded to the server' : 'Streaming from the podcast'" data-testid="np-episode-source">{{ episode.episode.downloaded ? "Downloaded" : "Streaming" }}</UiBadge>
      <UiBadge variant="accent" :title="`Audio chain: ${player.chain ?? '—'}`">{{ audioPath }}</UiBadge>
    </div>

    <div class="mb-4 flex flex-wrap items-center gap-2">
      <UiButton data-testid="np-episode-details" @click="nav.go('episode', episode.episode.id)"><Info /> Episode details</UiButton>
      <UiButton data-testid="np-up-next" @click="nav.go('podcasts')"><ListEnd /> Up Next ({{ podcasts.upNext.length }})</UiButton>
      <UiButton v-if="podcasts.stash" data-testid="np-podcast-back-to-music" @click="podcasts.returnToMusic()"><Music /> Back to music</UiButton>
    </div>

    <StateMessage v-if="player.error" kind="error">{{ player.error }}</StateMessage>
  </div>
</div>
<div v-else :class="compact ? 'flex flex-col items-start gap-4' : 'flex flex-col items-start gap-8 min-[720px]:flex-row'">
  <div class="shrink-0">
    <Artwork :hash="artworkHash" :placeholder="radio.isPlaying ? 'radio' : 'music'" :size="compact ? 160 : 320" :radius="6" :alt="trackTitle(track)" />
  </div>
  <div class="min-w-0 flex-1">
    <h2 class="heading-1 m-0 mb-1" data-testid="np-title">{{ trackTitle(track) }}</h2>
    <p class="m-0 mb-0.5 text-base text-dim">{{ track.artist ?? "Unknown artist" }}</p>
    <p class="m-0 mb-4 text-sm text-faint">{{ track.album ?? "" }}</p>

    <div class="mb-5 flex flex-wrap gap-2">
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

    <div class="mb-4"><TrackMenu :track="track" layout="buttons" :show-play-next="false" /></div>

    <StateMessage v-if="player.error" kind="error">{{ player.error }}</StateMessage>
  </div>
</div>
  
</template>
