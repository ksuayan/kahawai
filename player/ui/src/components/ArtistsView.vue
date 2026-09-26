<script setup lang="ts">
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";

const lib = useLibraryStore();
const nav = useNavStore();
</script>

<template>
  <ViewShell title="Artists" :subtitle="`${lib.artists.length} artists`">
    <StateMessage v-if="lib.loading" kind="loading">Loading artists…</StateMessage>
    <StateMessage v-else-if="lib.error" kind="error">{{ lib.error }}</StateMessage>
    <StateMessage v-else-if="lib.sortedArtists.length === 0" kind="empty">No artists found.</StateMessage>
    <ul v-else class="m-0 flex list-none flex-col gap-0.5 p-0">
      <li v-for="artist in lib.sortedArtists" :key="artist.id">
        <button
          type="button"
          class="flex w-full items-center gap-3 rounded-md px-2.5 py-2 text-left hover:bg-hover"
          @click="nav.go('artist', artist.id)"
        >
          <span
            class="flex size-9 shrink-0 items-center justify-center rounded-full bg-active font-semibold text-dim"
            aria-hidden="true"
          >
            {{ artist.name.charAt(0).toUpperCase() }}
          </span>
          <span class="text-sm">{{ artist.name }}</span>
        </button>
      </li>
    </ul>
  </ViewShell>
</template>
