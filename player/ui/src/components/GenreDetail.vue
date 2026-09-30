<script setup lang="ts">
import { ChevronLeft, Play } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { fetchGenreTracks } from "../api";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import { isPlayable, type Track } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import TrackMenu from "./TrackMenu.vue";
import TrackRow from "./TrackRow.vue";

const props = defineProps<{ name: string }>();

/** Tracks per request: big genres hold thousands, so they load in pages. */
const PAGE = 200;

const lib = useLibraryStore();
const nav = useNavStore();
const queue = useQueueStore();

const tracks = ref<Track[]>([]);
const total = ref(0);
const page = ref(0);
const loading = ref(true);
const loadingMore = ref(false);
const error = ref<string | null>(null);

async function loadPage(): Promise<void> {
  const res = await fetchGenreTracks(props.name, page.value + 1, PAGE);
  page.value = res.page;
  total.value = res.total;
  tracks.value = [...tracks.value, ...res.items];
  lib.cacheTracks(res.items);
}

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  tracks.value = [];
  total.value = 0;
  page.value = 0;
  try {
    await loadPage();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

async function more(): Promise<void> {
  loadingMore.value = true;
  try {
    await loadPage();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loadingMore.value = false;
  }
}

onMounted(load);
watch(() => props.name, load);

const playable = computed(() => tracks.value.filter(isPlayable));
const remaining = computed(() => total.value - tracks.value.length);

function playAll(): void {
  if (playable.value.length > 0) void queue.playAll(playable.value, 0);
}

function playFrom(track: Track): void {
  const idx = playable.value.findIndex((t) => t.id === track.id);
  if (idx >= 0) void queue.playAll(playable.value, idx);
}
</script>

<template>
  <ViewShell>
    <UiButton variant="icon" class="mb-3" @click="nav.go('genres')"><ChevronLeft /> Genres</UiButton>
    <StateMessage v-if="loading" kind="loading">Loading {{ name }}…</StateMessage>
    <StateMessage v-else-if="error && tracks.length === 0" kind="error">{{ error }}</StateMessage>
    <div v-else>
      <header class="mb-5">
        <h2 class="heading-1 mb-1.5 mt-1">{{ name }}</h2>
        <p class="m-0 mb-1 text-dim">{{ total.toLocaleString("en-US") }} tracks</p>
        <div class="mt-3 flex items-center gap-2">
          <UiButton variant="primary" :disabled="playable.length === 0" @click="playAll">
            <Play class="fill-current" /> Play
          </UiButton>
          <TrackMenu :tracks="tracks" layout="buttons" />
        </div>
      </header>
      <div class="flex flex-col gap-0.5">
        <TrackRow
          v-for="t in tracks"
          :key="t.id"
          :track="t"
          :show-artwork="true"
          :artwork-hash="lib.albums.find((a) => a.id === t.album_id)?.artwork_hash"
          :current="queue.current?.id === t.id"
          @play="playFrom"
        />
      </div>
      <div v-if="remaining > 0" class="mt-4 flex items-center gap-3">
        <UiButton :disabled="loadingMore" @click="more">
          {{ loadingMore ? "Loading…" : `Show ${Math.min(PAGE, remaining)} more` }}
        </UiButton>
        <span class="text-xs text-dim">{{ remaining.toLocaleString("en-US") }} not shown yet</span>
      </div>
      <StateMessage v-if="error && tracks.length > 0" kind="error">{{ error }}</StateMessage>
    </div>
  </ViewShell>
</template>
