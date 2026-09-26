<script setup lang="ts">
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";
import AlbumCard from "./AlbumCard.vue";

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
  <ViewShell title="Albums" :subtitle="`${lib.albums.length} albums in library`">
    <StateMessage v-if="lib.loading" kind="loading">Loading albums…</StateMessage>
    <StateMessage v-else-if="lib.error" kind="error">{{ lib.error }}</StateMessage>
    <StateMessage v-else-if="lib.sortedAlbums.length === 0" kind="empty">No albums found.</StateMessage>
    <div v-else class="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-x-4 gap-y-5">
      <AlbumCard
        v-for="album in lib.sortedAlbums"
        :key="album.id"
        :album="album"
        :subtitle="subtitle(album)"
        @open="(id) => nav.go('album', id)"
      />
    </div>
  </ViewShell>
</template>
