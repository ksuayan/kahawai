<script setup lang="ts">
import { useMainScrollMemory } from "../lib/mainScroll";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";

const lib = useLibraryStore();
const nav = useNavStore();

useMainScrollMemory("genres", () => !lib.loading);

const count = (n: number) => n.toLocaleString("en-US");
</script>

<template>
  <ViewShell title="Genres" :subtitle="`${lib.genres.length} genres`">
    <StateMessage v-if="lib.loading" kind="loading">Loading genres…</StateMessage>
    <StateMessage v-else-if="lib.error" kind="error">{{ lib.error }}</StateMessage>
    <StateMessage v-else-if="lib.genres.length === 0" kind="empty">
      No genres yet. They come from your files' genre tags after a scan.
    </StateMessage>
    <ul v-else class="m-0 flex list-none flex-wrap gap-2 p-0">
      <li v-for="g in lib.genres" :key="g.name">
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
  </ViewShell>
</template>
