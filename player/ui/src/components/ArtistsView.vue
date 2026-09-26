<script setup lang="ts">
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";

const lib = useLibraryStore();
const nav = useNavStore();
</script>

<template>
  <div class="view">
    <h2>Artists</h2>
    <p class="sub">{{ lib.artists.length }} artists</p>
    <div v-if="lib.loading" class="spinner">Loading artists…</div>
    <div v-else-if="lib.error" class="error-banner">{{ lib.error }}</div>
    <div v-else-if="lib.sortedArtists.length === 0" class="empty">No artists found.</div>
    <ul v-else class="list">
      <li v-for="artist in lib.sortedArtists" :key="artist.id">
        <button class="row" @click="nav.go('artist', artist.id)">
          <span class="avatar" aria-hidden="true">{{ artist.name.charAt(0).toUpperCase() }}</span>
          <span class="name">{{ artist.name }}</span>
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.row {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  background: transparent;
  border: none;
  border-radius: 6px;
  padding: 8px 10px;
  text-align: left;
}

.row:hover {
  background: var(--bg-hover);
}

.avatar {
  width: 36px;
  height: 36px;
  border-radius: 50%;
  background: var(--bg-active);
  color: var(--text-dim);
  display: flex;
  align-items: center;
  justify-content: center;
  font-weight: 600;
  flex-shrink: 0;
}

.name {
  font-size: 14px;
}
</style>
