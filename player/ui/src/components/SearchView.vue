<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useLibraryStore } from "../stores/library";
import { useQueueStore } from "../stores/queue";
import { isPlayable, type Track } from "../types";
import TrackRow from "./TrackRow.vue";

const lib = useLibraryStore();
const queue = useQueueStore();
const input = ref<HTMLInputElement | null>(null);

onMounted(() => input.value?.focus());

function onInput(e: Event): void {
  lib.search((e.target as HTMLInputElement).value);
}

function playFrom(track: Track): void {
  const playable = lib.searchResults.filter(isPlayable);
  const idx = playable.findIndex((t) => t.id === track.id);
  if (idx >= 0) void queue.playAll(playable, idx);
}
</script>

<template>
  <div class="view">
    <h2>Search</h2>
    <p class="sub">Full-text search across titles, artists, albums</p>
    <input
      ref="input"
      type="search"
      class="search-box"
      placeholder="Search tracks…"
      :value="lib.searchQuery"
      @input="onInput"
    />
    <div v-if="lib.searching" class="spinner">Searching…</div>
    <div v-else-if="lib.searchQuery.trim() && lib.searchResults.length === 0" class="empty">
      No tracks match "{{ lib.searchQuery.trim() }}".
    </div>
    <div v-else class="tracks">
      <TrackRow
        v-for="t in lib.searchResults"
        :key="t.id"
        :track="t"
        :show-artwork="true"
        :artwork-hash="lib.albums.find((a) => a.id === t.album_id)?.artwork_hash"
        :current="queue.current?.id === t.id"
        @play="playFrom"
      />
    </div>
  </div>
</template>

<style scoped>
.search-box {
  width: 100%;
  max-width: 480px;
  margin-bottom: 16px;
  font-size: 14px;
  padding: 8px 12px;
}

.tracks {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
</style>
