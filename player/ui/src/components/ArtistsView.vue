<script setup lang="ts">
import { computed } from "vue";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useViewPrefsStore } from "../stores/viewPrefs";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";
import ListToolbar from "./ListToolbar.vue";
import VirtualGrid from "./VirtualGrid.vue";
import VirtualList from "./VirtualList.vue";

const lib = useLibraryStore();
const nav = useNavStore();
const view = useViewPrefsStore();

const ROW_HEIGHT = 52;
const SORTS = [
  { value: "name-asc" as const, label: "Artist (A–Z)" },
  { value: "name-desc" as const, label: "Artist (Z–A)" },
];

const artists = computed(() =>
  view.prefs.artistsSort === "name-asc" ? lib.sortedArtists : [...lib.sortedArtists].reverse(),
);
const initial = (name: string) => name.charAt(0).toUpperCase();
</script>

<template>
  <ViewShell title="Artists" :subtitle="`${lib.artists.length} artists`" width="full">
    <template #actions>
      <ListToolbar v-model:layout="view.prefs.artistsLayout" v-model:sort="view.prefs.artistsSort" :sort-options="SORTS" />
    </template>
    <StateMessage v-if="lib.loading" kind="loading">Loading artists…</StateMessage>
    <StateMessage v-else-if="lib.error" kind="error">{{ lib.error }}</StateMessage>
    <StateMessage v-else-if="artists.length === 0" kind="empty">No artists found.</StateMessage>
    <VirtualGrid
      v-else-if="view.prefs.artistsLayout === 'grid'"
      :items="artists"
      scroll-key="artists:grid"
      :min-cell-width="130"
      :text-block-height="28"
    >
      <template #item="{ item }">
        <button
          type="button"
          class="group flex w-full flex-col items-center rounded-lg p-0 text-center outline-none focus-visible:outline-2 focus-visible:outline-accent"
          @click="nav.go('artist', item.id)"
        >
          <span
            class="flex aspect-square w-full items-center justify-center rounded-full bg-active text-3xl font-semibold text-dim transition group-hover:brightness-110"
            aria-hidden="true"
          >
            {{ initial(item.name) }}
          </span>
          <span class="mt-2 w-full truncate text-sm">{{ item.name }}</span>
        </button>
      </template>
    </VirtualGrid>
    <VirtualList v-else :items="artists" scroll-key="artists" :row-height="ROW_HEIGHT" :get-key="(a) => a.id">
      <template #item="{ item }">
        <button
          type="button"
          class="flex h-full w-full items-center gap-3 rounded-md px-2.5 text-left hover:bg-hover"
          @click="nav.go('artist', item.id)"
        >
          <span
            class="flex size-9 shrink-0 items-center justify-center rounded-full bg-active font-semibold text-dim"
            aria-hidden="true"
          >
            {{ initial(item.name) }}
          </span>
          <span class="text-sm">{{ item.name }}</span>
        </button>
      </template>
    </VirtualList>
  </ViewShell>
</template>
