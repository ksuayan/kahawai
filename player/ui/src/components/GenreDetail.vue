<script setup lang="ts">
import { ChevronLeft, Play } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { fetchGenreTracks } from "../api";
import { SORT_OPTIONS } from "../lib/sorting";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import { useViewPrefsStore } from "../stores/viewPrefs";
import { isPlayable, type Track } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import ListToolbar from "./ListToolbar.vue";
import TrackCollection from "./TrackCollection.vue";
import TrackMenu from "./TrackMenu.vue";

const props = defineProps<{ name: string }>();

/** Tracks per request: big genres hold thousands, so they load in pages
 *  as the list scrolls toward the end. */
const PAGE = 200;

const lib = useLibraryStore();
const nav = useNavStore();
const queue = useQueueStore();
const view = useViewPrefsStore();

const tracks = ref<Track[]>([]);
const total = ref(0);
const page = ref(0);
const loading = ref(true);
const loadingMore = ref(false);
const error = ref<string | null>(null);
/** Bumped on every reload, so a page that arrives late for an old genre or
 *  sort order is dropped. */
let generation = 0;

async function loadPage(gen: number): Promise<void> {
  const res = await fetchGenreTracks(props.name, page.value + 1, PAGE, view.prefs.genreTracksSort);
  if (gen !== generation) return;
  page.value = res.page;
  total.value = res.total;
  tracks.value = [...tracks.value, ...res.items];
  lib.cacheTracks(res.items);
}

async function load(): Promise<void> {
  const gen = ++generation;
  loading.value = true;
  error.value = null;
  tracks.value = [];
  total.value = 0;
  page.value = 0;
  try {
    await loadPage(gen);
  } catch (e) {
    if (gen === generation) error.value = e instanceof Error ? e.message : String(e);
  } finally {
    if (gen === generation) loading.value = false;
  }
}

const remaining = computed(() => total.value - tracks.value.length);

/** Scrolled near the end: fetch the next page, one at a time. */
async function more(): Promise<void> {
  if (loadingMore.value || loading.value || remaining.value <= 0 || error.value) return;
  const gen = generation;
  loadingMore.value = true;
  try {
    await loadPage(gen);
  } catch (e) {
    if (gen === generation) error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loadingMore.value = false;
  }
}

onMounted(load);
watch(() => props.name, load);
watch(() => view.prefs.genreTracksSort, load);

const playable = computed(() => tracks.value.filter(isPlayable));

function playAll(): void {
  if (playable.value.length > 0) void queue.playAll(playable.value, 0);
}

function playFrom(track: Track): void {
  const idx = playable.value.findIndex((t) => t.id === track.id);
  if (idx >= 0) void queue.playAll(playable.value, idx);
}
</script>

<template>
  <ViewShell width="full">
    <div class="shrink-0">
      <UiButton variant="icon" class="mb-3" @click="nav.go('genres')"><ChevronLeft /> Genres</UiButton>
    </div>
    <StateMessage v-if="loading" kind="loading">Loading {{ name }}…</StateMessage>
    <StateMessage v-else-if="error && tracks.length === 0" kind="error">{{ error }}</StateMessage>
    <template v-else>
      <header class="mb-4 flex shrink-0 flex-wrap items-end justify-between gap-4">
        <div>
          <h2 class="heading-1 mb-1.5 mt-1">{{ name }}</h2>
          <p class="m-0 mb-1 text-dim">{{ total.toLocaleString("en-US") }} tracks</p>
          <div class="mt-3 flex items-center gap-2">
            <UiButton variant="primary" :disabled="playable.length === 0" @click="playAll">
              <Play class="fill-current" /> Play
            </UiButton>
            <TrackMenu :tracks="tracks" layout="buttons" />
          </div>
        </div>
        <ListToolbar
          v-model:layout="view.prefs.genreTracksLayout"
          v-model:sort="view.prefs.genreTracksSort"
          :sort-options="SORT_OPTIONS"
        />
      </header>
      <TrackCollection
        :items="tracks"
        :track="(t) => t"
        :layout="view.prefs.genreTracksLayout"
        :scroll-key="`genre:${name}:${view.prefs.genreTracksSort}`"
        :is-current="(t) => queue.current?.id === t.id"
        @play="playFrom"
        @near-end="more"
      >
        <template #footer>
          <p v-if="loadingMore" class="px-2.5 py-3 text-xs text-dim" data-testid="loading-more">Loading more…</p>
          <StateMessage v-else-if="error" kind="error">{{ error }}</StateMessage>
        </template>
      </TrackCollection>
    </template>
  </ViewShell>
</template>
