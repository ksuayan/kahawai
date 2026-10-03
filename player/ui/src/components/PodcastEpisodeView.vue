<script setup lang="ts">
import { ArrowDownToLine, Check, ExternalLink, ListEnd, ListX, LoaderCircle, Trash2, Undo2, X } from "lucide-vue-next";
import { computed, onMounted, watch } from "vue";
import { clock, dayLine, duration, groupByDay, remainingText } from "../lib/audiobook";
import { usePlayToggle } from "../lib/playToggle";
import { episodeDate, episodeOfTrack, episodeProgress, sizeText } from "../lib/podcast";
import { useNavStore } from "../stores/nav";
import { usePodcastsStore } from "../stores/podcasts";
import { openUrl } from "../tauri";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import ShowNotes from "./ShowNotes.vue";

/** One episode: its show notes, where you are in it, when you listened, and its actions. */
const props = defineProps<{ id: number }>();
const podcasts = usePodcastsStore();
const nav = useNavStore();

onMounted(() => void podcasts.openEpisode(props.id));
watch(() => props.id, (id) => void podcasts.openEpisode(id));

const ep = computed(() => (podcasts.detail?.id === props.id ? podcasts.detail : null));
const playingThis = computed(() => podcasts.isActive && podcasts.active?.episode.id === props.id);
const offset = computed(() => (playingThis.value ? podcasts.offsetMs : (ep.value?.position_ms ?? 0)));
const played = computed(() => ep.value?.played_at != null);
const job = computed(() => podcasts.downloadJobs.get(props.id) ?? null);
const days = computed(() => groupByDay(podcasts.history));
const playButton = usePlayToggle((t) => episodeOfTrack(t.id) === props.id, () => (ep.value ? podcasts.play(ep.value) : undefined));
const progress = computed(() => (ep.value ? episodeProgress({ position_ms: offset.value, duration_ms: ep.value.duration_ms }) : 0));
const numbering = computed(() => {
  const e = ep.value;
  if (!e) return "";
  return [e.season != null ? `Season ${e.season}` : null, e.episode != null ? `Episode ${e.episode}` : null].filter(Boolean).join(", ");
});
</script>

<template>
  <ViewShell width="medium" section="podcasts" :crumb="ep?.title">
    <StateMessage v-if="podcasts.error && !ep" kind="error">{{ podcasts.error }}</StateMessage>
    <StateMessage v-else-if="!ep" kind="loading">Loading…</StateMessage>
    <template v-else>
      <header class="flex flex-col gap-4 min-[720px]:flex-row min-[720px]:gap-5">
        <Artwork :url="ep.image_url || ep.feed.image_url" placeholder="podcast" :size="160" :radius="8" :alt="ep.feed.title" />
        <div class="min-w-0 flex-1">
          <button type="button" class="border-0 bg-transparent p-0 text-left text-[13px] text-accent hover:underline" data-testid="episode-show" @click="nav.go('podcast', ep.feed.id)">{{ ep.feed.title }}</button>
          <h2 class="heading-1 m-0 mb-1 mt-1" data-testid="episode-heading">{{ ep.title }}</h2>
          <p class="m-0 text-xs text-dim" data-testid="episode-facts">
            {{ [episodeDate(ep.published_at), numbering, ep.duration_ms ? duration(ep.duration_ms) : ""].filter(Boolean).join(" · ") }}
            <template v-if="offset > 0 && !played && ep.duration_ms"> · {{ remainingText(ep.duration_ms, offset, playingThis ? podcasts.speed : ep.feed.speed) }}</template>
            <template v-if="played"> · played</template>
          </p>
          <div v-if="offset > 0 && !played" class="mt-2 h-1.5 w-full max-w-[420px] overflow-hidden rounded-full bg-active" data-testid="episode-detail-progress">
            <div class="h-full bg-accent" :style="{ width: `${Math.round(progress * 100)}%` }" />
          </div>
          <div class="mt-4 flex flex-wrap items-center gap-2">
            <UiButton variant="primary" data-testid="episode-detail-play" @click="playButton.press()">
              <component :is="playButton.icon" class="fill-current" />
              {{ playButton.playing ? "Pause" : offset > 0 && !played ? `Continue from ${clock(offset)}` : "Play" }}
            </UiButton>
            <UiButton
              data-testid="episode-detail-up-next"
              @click="podcasts.inUpNext(ep.id) ? podcasts.removeFromUpNext(ep.id) : podcasts.addToUpNext(ep)"
            >
              <ListX v-if="podcasts.inUpNext(ep.id)" /><ListEnd v-else /> {{ podcasts.inUpNext(ep.id) ? "Remove from Up Next" : "Add to Up Next" }}
            </UiButton>
            <UiButton v-if="job" data-testid="episode-detail-cancel" @click="podcasts.removeDownload(ep)"><LoaderCircle class="animate-spin" /> Downloading {{ Math.round(job.progress * 100) }}% <X /></UiButton>
            <UiButton v-else-if="ep.downloaded" data-testid="episode-detail-delete" @click="podcasts.removeDownload(ep)"><Trash2 /> Delete download{{ ep.file_bytes ? ` (${sizeText(ep.file_bytes)})` : "" }}</UiButton>
            <UiButton v-else data-testid="episode-detail-download" @click="podcasts.download(ep)"><ArrowDownToLine /> Download</UiButton>
            <UiButton data-testid="episode-detail-mark" @click="podcasts.setPlayed(ep, !played)">
              <Undo2 v-if="played" /><Check v-else /> {{ played ? "Mark unplayed" : "Mark played" }}
            </UiButton>
            <UiButton v-if="ep.link" variant="icon" title="The episode's page" aria-label="The episode's page" @click="openUrl(ep.link)"><ExternalLink /></UiButton>
          </div>
          <p v-if="ep.dropped_from_feed" class="m-0 mt-2 text-xs text-faint">No longer listed in the feed; kept here.</p>
        </div>
      </header>

      <section class="mt-6" aria-label="Show notes">
        <h3 class="heading-3 mb-2">Show notes</h3>
        <ShowNotes :html="ep.description_html" />
      </section>

      <section v-if="days.length" class="mt-6" aria-label="Listening history">
        <h3 class="heading-3 mb-2">Listening history</h3>
        <ul class="m-0 list-none p-0 text-[13px]">
          <li v-for="d in days" :key="d.day" class="py-0.5 text-dim" data-testid="episode-history-day">{{ dayLine(d) }}</li>
        </ul>
      </section>
    </template>
  </ViewShell>
</template>
