<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useLibraryStore } from "../stores/library";
import { useQueueStore } from "../stores/queue";
import { isPlayable, type Track } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiInput from "../ui/UiInput.vue";
import ViewShell from "../ui/ViewShell.vue";
import TrackRow from "./TrackRow.vue";

const lib = useLibraryStore();
const queue = useQueueStore();
const input = ref<InstanceType<typeof UiInput> | null>(null);

onMounted(() => input.value?.focus());

function playFrom(track: Track): void {
  const playable = lib.searchResults.filter(isPlayable);
  const idx = playable.findIndex((t) => t.id === track.id);
  if (idx >= 0) void queue.playAll(playable, idx);
}
</script>

<template>
  <ViewShell title="Search" subtitle="Full-text search across titles, artists, albums">
    <UiInput
      ref="input"
      type="search"
      size="lg" class="mb-4 w-full max-w-[480px]"
      placeholder="Search tracks…"
      aria-label="Search tracks"
      :model-value="lib.searchQuery"
      @update:model-value="(v) => lib.search(v)"
    />
    <StateMessage v-if="lib.searching" kind="loading">Searching…</StateMessage>
    <StateMessage v-else-if="lib.searchQuery.trim() && lib.searchResults.length === 0" kind="empty">
      No tracks match "{{ lib.searchQuery.trim() }}".
    </StateMessage>
    <div v-else class="flex flex-col gap-0.5">
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
  </ViewShell>
</template>
