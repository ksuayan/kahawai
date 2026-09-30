<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { sortTracks, trackSortOptions } from "../lib/sorting";
import { useLibraryStore } from "../stores/library";
import { useQueueStore } from "../stores/queue";
import { useViewPrefsStore } from "../stores/viewPrefs";
import { isPlayable, type Track } from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiInput from "../ui/UiInput.vue";
import ViewShell from "../ui/ViewShell.vue";
import ListToolbar from "./ListToolbar.vue";
import TrackCollection from "./TrackCollection.vue";

const lib = useLibraryStore();
const queue = useQueueStore();
const view = useViewPrefsStore();
const input = ref<InstanceType<typeof UiInput> | null>(null);
const SORTS = trackSortOptions("Relevance");

onMounted(() => input.value?.focus());

/** Results in the chosen order ("Relevance" is the server's ranking). */
const shown = computed(() => sortTracks(lib.searchResults, view.prefs.searchSort, (t) => t));

function playFrom(track: Track): void {
  const playable = shown.value.filter(isPlayable);
  const idx = playable.findIndex((t) => t.id === track.id);
  if (idx >= 0) void queue.playAll(playable, idx);
}
</script>

<template>
  <ViewShell title="Search" subtitle="Full-text search across titles, artists, albums and genres" width="full">
    <template #actions>
      <ListToolbar v-model:layout="view.prefs.searchLayout" v-model:sort="view.prefs.searchSort" :sort-options="SORTS" />
    </template>
    <div class="shrink-0">
      <UiInput
        ref="input"
        type="search"
        size="lg" class="mb-4 w-full max-w-[480px]"
        placeholder="Search tracks…"
        aria-label="Search tracks"
        :model-value="lib.searchQuery"
        @update:model-value="(v) => lib.search(v)"
      />
    </div>
    <StateMessage v-if="lib.searching" kind="loading">Searching…</StateMessage>
    <StateMessage v-else-if="lib.searchQuery.trim() && lib.searchResults.length === 0" kind="empty">
      No tracks match "{{ lib.searchQuery.trim() }}".
    </StateMessage>
    <TrackCollection
      v-else-if="shown.length > 0"
      :items="shown"
      :track="(t) => t"
      :layout="view.prefs.searchLayout"
      scroll-key="search"
      :is-current="(t) => queue.current?.id === t.id"
      @play="playFrom"
    />
  </ViewShell>
</template>
