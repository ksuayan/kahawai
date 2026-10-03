<script setup lang="ts">
import { ExternalLink, RefreshCw, Settings, Trash2, TriangleAlert } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { usePlayToggle } from "../lib/playToggle";
import { episodeDate, episodeOfTrack } from "../lib/podcast";
import { useNavStore } from "../stores/nav";
import { usePodcastsStore } from "../stores/podcasts";
import { openUrl } from "../tauri";
import ConfirmDialog from "../ui/ConfirmDialog.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiSwitch from "../ui/UiSwitch.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import EpisodeRow from "./EpisodeRow.vue";
import PodcastSettingsDialog from "./PodcastSettingsDialog.vue";

/** One show: what it is, whether its feed is being read, and its episodes newest first. */
const props = defineProps<{ id: number }>();
const podcasts = usePodcastsStore();
const nav = useNavStore();

const settingsOpen = ref(false);
const confirming = ref(false);

onMounted(() => void podcasts.loadEpisodes(props.id));
watch(() => props.id, (id) => void podcasts.loadEpisodes(id));
watch(() => podcasts.unplayedOnly, () => void podcasts.loadEpisodes(props.id));

const feed = computed(() => podcasts.feedById(props.id));
const episodes = computed(() => (podcasts.episodesFeed === props.id ? podcasts.episodes : []));
/** The newest unplayed episode: what Play starts. */
const latest = computed(() => episodes.value.find((e) => e.played_at == null) ?? null);
const playButton = usePlayToggle((t) => episodeOfTrack(t.id) !== null && podcasts.active?.feed.id === props.id, () => (latest.value ? podcasts.play(latest.value) : undefined));

async function unsubscribe(): Promise<void> {
  await podcasts.unsubscribe(props.id);
  nav.go("podcasts");
}
</script>

<template>
  <ViewShell width="wide" section="podcasts" :crumb="feed?.title">
    <StateMessage v-if="!feed && podcasts.error" kind="error">{{ podcasts.error }}</StateMessage>
    <StateMessage v-else-if="!feed" kind="loading">Loading…</StateMessage>
    <template v-else>
      <header class="mb-5 flex gap-5">
        <Artwork :url="feed.image_url" placeholder="podcast" :size="180" :radius="8" :alt="feed.title" />
        <div class="min-w-0 flex-1">
          <h2 class="heading-1 m-0 mb-1" data-testid="podcast-title">{{ feed.title }}</h2>
          <p v-if="feed.author" class="m-0 text-dim">{{ feed.author }}</p>
          <p class="m-0 mt-1 text-xs text-faint" data-testid="podcast-counts">
            {{ feed.episode_count }} episodes · {{ feed.unplayed_count }} unplayed<template v-if="feed.last_fetched"> · checked {{ episodeDate(feed.last_fetched) }}</template>
          </p>
          <p v-if="feed.description" class="m-0 mt-2 line-clamp-3 max-w-[70ch] text-[13px] text-dim" data-testid="podcast-description">{{ feed.description }}</p>
          <p v-if="feed.last_error" class="m-0 mt-2 flex items-start gap-1.5 text-xs text-danger-fg" role="alert" data-testid="podcast-error">
            <TriangleAlert class="mt-px size-3.5 shrink-0" /> The feed couldn't be read last time: {{ feed.last_error }}. Its episodes are kept.
          </p>
          <div class="mt-4 flex flex-wrap items-center gap-2">
            <UiButton variant="primary" :disabled="!latest && !playButton.playing" data-testid="podcast-play" @click="playButton.press()">
              <component :is="playButton.icon" class="fill-current" /> {{ playButton.playing ? "Pause" : latest ? "Play latest" : "Play" }}
            </UiButton>
            <UiButton data-testid="podcast-refresh" @click="podcasts.refresh(feed.id)"><RefreshCw /> Check for new</UiButton>
            <UiButton data-testid="podcast-settings-open" @click="settingsOpen = true"><Settings /> Settings</UiButton>
            <UiButton v-if="feed.link" variant="icon" title="The show's website" aria-label="The show's website" @click="openUrl(feed.link)"><ExternalLink /></UiButton>
            <UiButton variant="icon-danger" title="Unsubscribe" aria-label="Unsubscribe" data-testid="podcast-unsubscribe" @click="confirming = true"><Trash2 /></UiButton>
          </div>
        </div>
      </header>

      <div class="mb-2 flex items-center justify-between">
        <h3 class="heading-3 m-0">Episodes</h3>
        <UiSwitch v-model="podcasts.unplayedOnly" label="Unplayed only" />
      </div>
      <StateMessage v-if="episodes.length === 0" kind="empty">{{ podcasts.unplayedOnly ? "Everything is played." : "No episodes yet." }}</StateMessage>
      <ul v-else class="m-0 list-none p-0" data-testid="podcast-episodes">
        <EpisodeRow v-for="e in episodes" :key="e.id" :episode="e" />
      </ul>

      <PodcastSettingsDialog v-model:open="settingsOpen" :feed="feed" />
      <ConfirmDialog
        v-model:open="confirming"
        :title="`Unsubscribe from ${feed.title}?`"
        description="Its episodes, places and downloaded files are removed from the server."
        confirm-label="Unsubscribe"
        danger
        @confirm="unsubscribe"
      />
    </template>
  </ViewShell>
</template>
