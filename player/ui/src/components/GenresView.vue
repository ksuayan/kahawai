<script setup lang="ts">
import { computed } from "vue";
import { useMainScrollMemory } from "../lib/mainScroll";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useViewPrefsStore } from "../stores/viewPrefs";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";
import ListToolbar from "./ListToolbar.vue";

const lib = useLibraryStore();
const nav = useNavStore();
const view = useViewPrefsStore();

// A few dozen genres: no virtualization needed.
useMainScrollMemory("genres", () => !lib.loading);

const SORTS = [
  { value: "count-desc" as const, label: "Most tracks first" },
  { value: "count-asc" as const, label: "Fewest tracks first" },
  { value: "name-asc" as const, label: "Genre (A–Z)" },
  { value: "name-desc" as const, label: "Genre (Z–A)" },
];

const genres = computed(() => {
  const list = [...lib.genres];
  const byName = (a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name);
  switch (view.prefs.genresSort) {
    case "count-asc":
      return list.sort((a, b) => a.track_count - b.track_count || byName(a, b));
    case "name-asc":
      return list.sort(byName);
    case "name-desc":
      return list.sort((a, b) => byName(b, a));
    default:
      return list.sort((a, b) => b.track_count - a.track_count || byName(a, b));
  }
});

const count = (n: number) => n.toLocaleString("en-US");
</script>

<template>
  <ViewShell title="Genres" :subtitle="`${lib.genres.length} genres`">
    <template #actions>
      <ListToolbar v-model:layout="view.prefs.genresLayout" v-model:sort="view.prefs.genresSort" :sort-options="SORTS" />
    </template>
    <StateMessage v-if="lib.loading" kind="loading">Loading genres…</StateMessage>
    <StateMessage v-else-if="lib.error" kind="error">{{ lib.error }}</StateMessage>
    <StateMessage v-else-if="genres.length === 0" kind="empty">
      No genres yet. They come from your files' genre tags after a scan.
    </StateMessage>
    <ul v-else-if="view.prefs.genresLayout === 'grid'" class="m-0 flex list-none flex-wrap gap-2 p-0">
      <li v-for="g in genres" :key="g.name">
        <button
          type="button"
          class="flex items-baseline gap-2 rounded-full border border-line bg-raised px-3.5 py-1.5 text-sm hover:bg-hover"
          @click="nav.go('genre', undefined, g.name)"
        >
          {{ g.name }}
          <span class="text-xs tabular-nums text-dim">{{ count(g.track_count) }}</span>
        </button>
      </li>
    </ul>
    <ul v-else class="m-0 flex list-none flex-col gap-0.5 p-0">
      <li v-for="g in genres" :key="g.name">
        <button
          type="button"
          class="flex w-full items-center justify-between gap-3 rounded-md px-2.5 py-2 text-left text-sm hover:bg-hover"
          @click="nav.go('genre', undefined, g.name)"
        >
          {{ g.name }}
          <span class="text-xs tabular-nums text-dim">{{ count(g.track_count) }} tracks</span>
        </button>
      </li>
    </ul>
  </ViewShell>
</template>
