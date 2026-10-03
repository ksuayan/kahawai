<script setup lang="ts">
import { ArrowDownToLine, Check, ListEnd, ListX, LoaderCircle, Trash2, Undo2, X } from "lucide-vue-next";
import { ContextMenuContent, ContextMenuItem, ContextMenuPortal, ContextMenuRoot, ContextMenuSeparator, ContextMenuTrigger } from "reka-ui";
import { computed } from "vue";
import { clock, duration } from "../lib/audiobook";
import { joinParts } from "../lib/format";
import { usePlayToggle } from "../lib/playToggle";
import { episodeDate, episodeOfTrack, episodeProgress, sizeText } from "../lib/podcast";
import { useNavStore } from "../stores/nav";
import { usePodcastsStore } from "../stores/podcasts";
import type { PodcastEpisode } from "../types";
import UiButton from "../ui/UiButton.vue";
import Artwork from "./Artwork.vue";

/**
 * One episode in a list: play/pause, title (opens the episode), date and
 * length, how far you are, the download state (downloaded with its size,
 * downloading with progress, or streamed), and its actions.
 */
const props = withDefaults(defineProps<{ episode: PodcastEpisode; showFeed?: boolean; showArt?: boolean }>(), { showFeed: false, showArt: false });
const podcasts = usePodcastsStore();
const nav = useNavStore();

const ep = computed(() => props.episode);
const playButton = usePlayToggle((t) => episodeOfTrack(t.id) === ep.value.id, () => podcasts.play(ep.value));
const job = computed(() => podcasts.downloadJobs.get(ep.value.id) ?? null);
const progress = computed(() => episodeProgress(ep.value));
const played = computed(() => ep.value.played_at != null);
const itemClass =
  "flex cursor-default select-none items-center justify-between gap-3 whitespace-nowrap rounded-md px-2.5 py-2 text-[13px] text-fg outline-none " +
  "data-[disabled]:opacity-40 data-[highlighted]:bg-hover";
const contentClass = "z-[60] min-w-[190px] rounded-md border border-line bg-surface p-1 shadow-float";
const meta = computed(() => {
  const e = ep.value;
  const parts = [episodeDate(e.published_at)];
  if (e.duration_ms) {
    parts.push(e.position_ms > 0 && !played.value ? `${clock(e.duration_ms - e.position_ms)} left` : duration(e.duration_ms));
  }
  if (props.showFeed) parts.unshift(e.feed_title);
  return joinParts(parts);
});
</script>

<template>
  <!-- Right-click: Play, Up Next, Mark played, Delete (the download). -->
  <ContextMenuRoot>
    <ContextMenuTrigger as-child>
  <li class="flex items-center gap-3 rounded-md px-2 py-2 hover:bg-hover" :class="played && 'opacity-70'" data-testid="episode-row" :data-played="played || undefined">
    <UiButton variant="icon" :title="playButton.label" :aria-label="`${playButton.label} ${ep.title}`" data-testid="episode-play" @click="playButton.press()">
      <component :is="playButton.icon" class="fill-current" />
    </UiButton>
    <Artwork v-if="showArt" :url="ep.image_url || ep.feed_image_url" placeholder="podcast" :size="40" :radius="4" :alt="ep.feed_title" />
    <div class="min-w-0 flex-1">
      <button type="button" class="block max-w-full truncate border-0 bg-transparent p-0 text-left font-semibold text-fg hover:underline" data-testid="episode-title" @click="nav.go('episode', ep.id)">
        {{ ep.title }}
      </button>
      <div class="flex items-center gap-2 text-xs text-dim">
        <span class="truncate" data-testid="episode-meta">{{ meta }}</span>
        <span v-if="played" class="flex shrink-0 items-center gap-0.5 text-faint" data-testid="episode-played"><Check class="size-3" /> Played</span>
      </div>
      <div v-if="progress > 0 && !played" class="mt-1 h-1 w-full max-w-[240px] overflow-hidden rounded-full bg-active" data-testid="episode-progress">
        <div class="h-full bg-accent" :style="{ width: `${Math.round(progress * 100)}%` }" />
      </div>
    </div>

    <span class="hidden w-36 shrink-0 text-right text-xs text-faint min-[760px]:block" data-testid="episode-download-state">
      <template v-if="job"><LoaderCircle class="inline size-3 animate-spin" /> Downloading {{ Math.round(job.progress * 100) }}%</template>
      <template v-else-if="ep.downloaded">Downloaded{{ ep.file_bytes ? ` · ${sizeText(ep.file_bytes)}` : "" }}</template>
      <template v-else>Streams</template>
    </span>
    <span class="flex shrink-0 gap-0.5">
      <UiButton v-if="job" variant="icon" title="Cancel the download" aria-label="Cancel the download" data-testid="episode-cancel" @click="podcasts.removeDownload(ep)"><X /></UiButton>
      <UiButton v-else-if="ep.downloaded" variant="icon-danger" title="Delete the download (the episode stays)" aria-label="Delete the download" data-testid="episode-delete" @click="podcasts.removeDownload(ep)"><Trash2 /></UiButton>
      <UiButton v-else variant="icon" title="Download to the server" aria-label="Download" data-testid="episode-download" @click="podcasts.download(ep)"><ArrowDownToLine /></UiButton>
      <UiButton
        variant="icon"
        :title="podcasts.inUpNext(ep.id) ? 'Remove from Up Next' : 'Add to Up Next'"
        :aria-label="podcasts.inUpNext(ep.id) ? 'Remove from Up Next' : 'Add to Up Next'"
        data-testid="episode-up-next"
        @click="podcasts.inUpNext(ep.id) ? podcasts.removeFromUpNext(ep.id) : podcasts.addToUpNext(ep)"
      >
        <ListX v-if="podcasts.inUpNext(ep.id)" /><ListEnd v-else />
      </UiButton>
      <UiButton
        variant="icon"
        :title="played ? 'Mark unplayed' : 'Mark played'"
        :aria-label="played ? 'Mark unplayed' : 'Mark played'"
        data-testid="episode-mark"
        @click="podcasts.setPlayed(ep, !played)"
      >
        <Undo2 v-if="played" /><Check v-else />
      </UiButton>
    </span>
  </li>
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent :class="contentClass" data-kw-fade data-testid="episode-menu">
        <ContextMenuItem :class="itemClass" data-testid="episode-menu-play" @select="playButton.press()">
          <span class="flex items-center gap-2"><component :is="playButton.icon" class="size-4 text-dim" />{{ playButton.label }}</span>
        </ContextMenuItem>
        <ContextMenuItem
          :class="itemClass"
          data-testid="episode-menu-up-next"
          @select="podcasts.inUpNext(ep.id) ? podcasts.removeFromUpNext(ep.id) : podcasts.addToUpNext(ep)"
        >
          <span class="flex items-center gap-2">
            <ListX v-if="podcasts.inUpNext(ep.id)" class="size-4 text-dim" /><ListEnd v-else class="size-4 text-dim" />
            {{ podcasts.inUpNext(ep.id) ? "Remove from Up Next" : "Add to Up Next" }}
          </span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" data-testid="episode-menu-mark" @select="podcasts.setPlayed(ep, !played)">
          <span class="flex items-center gap-2"><Undo2 v-if="played" class="size-4 text-dim" /><Check v-else class="size-4 text-dim" />{{ played ? "Mark unplayed" : "Mark played" }}</span>
        </ContextMenuItem>
        <ContextMenuSeparator class="my-1 h-px bg-line" />
        <ContextMenuItem :class="itemClass" :disabled="!ep.downloaded && !job" data-testid="episode-menu-delete" @select="podcasts.removeDownload(ep)">
          <span class="flex items-center gap-2 text-danger-fg"><Trash2 class="size-4" />{{ job ? "Cancel download" : "Delete download" }}</span>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
</template>
