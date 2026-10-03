<script setup lang="ts">
import { usePlayToggle } from "../lib/playToggle";
import { joinParts } from "../lib/format";
import { computed, onMounted, ref, watch } from "vue";
import { sortTracks, trackSortOptions } from "../lib/sorting";
import { useLibraryStore } from "../stores/library";
import { useQueueStore } from "../stores/queue";
import { useViewPrefsStore } from "../stores/viewPrefs";
import { formatDuration, isPlayable, type Album, type Track } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import ListToolbar from "./ListToolbar.vue";
import TrackCollection from "./TrackCollection.vue";
import TrackMenu from "./TrackMenu.vue";

const props = defineProps<{ id: number }>();

const lib = useLibraryStore();
const queue = useQueueStore();
const view = useViewPrefsStore();
const SORTS = trackSortOptions("Track number");

const album = ref<Album | null>(null);
const tracks = ref<Track[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);

async function load(id: number): Promise<void> {
  loading.value = true;
  error.value = null;
  album.value = null;
  tracks.value = [];
  try {
    const detail = await lib.getAlbumDetail(id);
    album.value = detail.album;
    tracks.value = detail.tracks;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(() => load(props.id));
watch(() => props.id, (id) => load(id));

/** The tracks in the chosen order: Play and play-from follow what's shown. */
const shown = computed(() => sortTracks(tracks.value, view.prefs.albumTracksSort, (t) => t));
const playableTracks = () =>
  shown.value.map((t, i) => ({ t, i })).filter(({ t }) => isPlayable(t));

function playAll(): void {
  const list = playableTracks();
  if (list.length > 0) void queue.playAll(list.map((x) => x.t), 0);
}
/** Play, or Pause while one of this album's tracks plays. */
const playButton = usePlayToggle((t) => tracks.value.some((x) => x.id === t.id), playAll);

function playFrom(track: Track): void {
  const list = playableTracks().map((x) => x.t);
  const idx = list.findIndex((t) => t.id === track.id);
  if (idx >= 0) void queue.playAll(list, idx);
}

function totalDuration(): string {
  const ms = tracks.value.reduce((s, t) => s + (t.duration_ms ?? 0), 0);
  return formatDuration(ms);
}
</script>

<template>
  <ViewShell width="full" section="albums" :crumb="album?.title">
    <StateMessage v-if="loading" kind="loading">Loading album…</StateMessage>
    <StateMessage v-else-if="error" kind="error">{{ error }}</StateMessage>
    <template v-else-if="album">
      <header class="mb-5 flex shrink-0 flex-wrap items-end justify-between gap-5">
        <div class="detail-hero">
          <Artwork :hash="album.artwork_hash" :size="180" :radius="6" :alt="album.title" />
          <div>
            <h2 class="heading-1 mb-1.5 mt-1">{{ album.title }}</h2>
            <p class="m-0 mb-1 text-dim">
              {{ joinParts([album.artist, album.year ? String(album.year) : null]) }}
            </p>
            <p class="m-0 mb-1 text-dim">{{ tracks.length }} tracks · {{ totalDuration() }}</p>
            <div class="mt-3 flex items-center gap-2">
              <UiButton variant="primary" :disabled="playableTracks().length === 0" data-testid="play-all" @click="playButton.press()">
                <component :is="playButton.icon" class="fill-current" /> {{ playButton.label }}
              </UiButton>
              <TrackMenu :tracks="tracks" :album-id="album.id" layout="buttons" />
            </div>
          </div>
        </div>
        <ListToolbar
          v-model:layout="view.prefs.albumTracksLayout"
          v-model:sort="view.prefs.albumTracksSort"
          :sort-options="SORTS"
        />
      </header>
      <TrackCollection
        :items="shown"
        :track="(t) => t"
        :layout="view.prefs.albumTracksLayout"
        :scroll-key="`album:${album.id}`"
        :is-current="(t) => queue.current?.id === t.id"
        :number="(t) => t.track_no ?? null"
        @play="playFrom"
      />
    </template>
  </ViewShell>
</template>
