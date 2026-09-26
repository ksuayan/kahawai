<script setup lang="ts">
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import Artwork from "./Artwork.vue";

const lib = useLibraryStore();
const nav = useNavStore();

function subtitle(a: { artist?: string | null; year?: number | null; track_count: number }): string {
  const parts: string[] = [];
  if (a.artist) parts.push(a.artist);
  if (a.year) parts.push(String(a.year));
  parts.push(`${a.track_count} track${a.track_count === 1 ? "" : "s"}`);
  return parts.join(" · ");
}
</script>

<template>
  <div class="view">
    <h2>Albums</h2>
    <p class="sub">{{ lib.albums.length }} albums in library</p>
    <div v-if="lib.loading" class="spinner">Loading albums…</div>
    <div v-else-if="lib.error" class="error-banner">{{ lib.error }}</div>
    <div v-else-if="lib.sortedAlbums.length === 0" class="empty">No albums found.</div>
    <div v-else class="grid">
      <button
        v-for="album in lib.sortedAlbums"
        :key="album.id"
        class="card"
        @click="nav.go('album', album.id)"
      >
        <Artwork :hash="album.artwork_hash" :size="160" :radius="8" :alt="album.title" class="cover" />
        <div class="title">{{ album.title }}</div>
        <div class="sub-line">{{ subtitle(album) }}</div>
      </button>
    </div>
  </div>
</template>

<style scoped>
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
  border-radius: 8px;
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
