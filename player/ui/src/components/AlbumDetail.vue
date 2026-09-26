<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useQueueStore } from "../stores/queue";
import { formatDuration, isPlayable, type Album, type Track } from "../types";
import Artwork from "./Artwork.vue";
import TrackMenu from "./TrackMenu.vue";
import TrackRow from "./TrackRow.vue";

const props = defineProps<{ id: number }>();

const lib = useLibraryStore();
const nav = useNavStore();
const queue = useQueueStore();

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

const playableTracks = () =>
  tracks.value.map((t, i) => ({ t, i })).filter(({ t }) => isPlayable(t));

function playAll(): void {
  const list = playableTracks();
  if (list.length > 0) void queue.playAll(list.map((x) => x.t), 0);
}

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
  <div class="view">
    <button class="back icon-btn" @click="nav.go('albums')">‹ Albums</button>
    <div v-if="loading" class="spinner">Loading album…</div>
    <div v-else-if="error" class="error-banner">{{ error }}</div>
    <div v-else-if="album">
      <header>
        <Artwork :hash="album.artwork_hash" :size="180" :radius="10" :alt="album.title" />
        <div class="meta">
          <h2>{{ album.title }}</h2>
          <p class="sub">
            {{ [album.artist, album.year ? String(album.year) : null].filter(Boolean).join(" · ") }}
          </p>
          <p class="sub">{{ tracks.length }} tracks · {{ totalDuration() }}</p>
          <div class="actions">
            <button class="primary" :disabled="playableTracks().length === 0" @click="playAll">
              ▶ Play
            </button>
            <TrackMenu :tracks="tracks" :album-id="album.id" />
          </div>
        </div>
      </header>
      <div class="tracks">
        <TrackRow
          v-for="t in tracks"
          :key="t.id"
          :track="t"
          :current="queue.current?.id === t.id"
          @play="playFrom"
        />
      </div>
    </div>
  </div>
</template>

<style scoped>
.back {
  margin-bottom: 12px;
  font-size: 13px;
}

header {
  display: flex;
  gap: 20px;
  margin-bottom: 20px;
}

.meta h2 {
  margin: 4px 0 6px;
}

.meta .sub {
  margin: 0 0 4px;
}

.actions {
  margin-top: 12px;
}

.tracks {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
</style>
