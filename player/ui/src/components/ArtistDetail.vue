<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import type { Album, Artist } from "../types";
import Artwork from "./Artwork.vue";

const props = defineProps<{ id: number }>();

const lib = useLibraryStore();
const nav = useNavStore();

const artist = ref<Artist | null>(null);
const albums = ref<Album[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);

async function load(id: number): Promise<void> {
  loading.value = true;
  error.value = null;
  artist.value = null;
  albums.value = [];
  try {
    const detail = await lib.getArtistAlbums(id);
    artist.value = detail.artist;
    albums.value = [...detail.albums].sort(
      (a, b) => (a.year ?? 0) - (b.year ?? 0) || a.title.localeCompare(b.title),
    );
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(() => load(props.id));
watch(() => props.id, (id) => load(id));
</script>

<template>
  <div class="view">
    <button class="back icon-btn" @click="nav.go('artists')">‹ Artists</button>
    <div v-if="loading" class="spinner">Loading artist…</div>
    <div v-else-if="error" class="error-banner">{{ error }}</div>
    <div v-else-if="artist">
      <h2>{{ artist.name }}</h2>
      <p class="sub">{{ albums.length }} album{{ albums.length === 1 ? "" : "s" }}</p>
      <div v-if="albums.length === 0" class="empty">No albums found for this artist.</div>
      <div v-else class="grid">
        <button
          v-for="album in albums"
          :key="album.id"
          class="card"
          @click="nav.go('album', album.id)"
        >
          <Artwork :hash="album.artwork_hash" :size="160" :radius="8" :alt="album.title" class="cover" />
          <div class="title">{{ album.title }}</div>
          <div class="sub-line">
            {{ [album.year ? String(album.year) : null, `${album.track_count} tracks`].filter(Boolean).join(" · ") }}
          </div>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.back {
  margin-bottom: 12px;
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 20px 16px;
}

.card {
  background: transparent;
  border: none;
  padding: 0;
  text-align: left;
  cursor: pointer;
}

.card:hover .cover {
  filter: brightness(1.08);
}

.cover {
  width: 100% !important;
  height: auto !important;
  aspect-ratio: 1;
}

.cover :deep(img) {
  border-radius: 8px;
}

.title {
  margin-top: 8px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sub-line {
  color: var(--text-dim);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
