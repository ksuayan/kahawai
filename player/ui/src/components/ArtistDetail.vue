<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { joinParts } from "../lib/format";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import type { Album, Artist } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";
import AlbumCard from "./AlbumCard.vue";

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
  <ViewShell width="fluid" section="artists" :crumb="artist?.name">
    <StateMessage v-if="loading" kind="loading">Loading artist…</StateMessage>
    <StateMessage v-else-if="error" kind="error">{{ error }}</StateMessage>
    <div v-else-if="artist">
      <h2 class="heading-1 m-0 mb-1">{{ artist.name }}</h2>
      <p class="m-0 mb-4 text-dim">{{ albums.length }} album{{ albums.length === 1 ? "" : "s" }}</p>
      <StateMessage v-if="albums.length === 0" kind="empty">No albums found for this artist.</StateMessage>
      <div v-else class="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-x-4 gap-y-5">
        <AlbumCard
          v-for="album in albums"
          :key="album.id"
          :album="album"
          :subtitle="joinParts([album.year ? String(album.year) : null, `${album.track_count} tracks`])"
          @open="(id) => nav.go('album', id)"
        />
      </div>
    </div>
  </ViewShell>
</template>
