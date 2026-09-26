<script setup lang="ts">
import type { Album } from "../types";
import Artwork from "./Artwork.vue";

/** One album tile in a grid: cover, title, and a caller-supplied subtitle. */
defineProps<{ album: Album; subtitle: string }>();
defineEmits<{ (e: "open", id: number): void }>();
</script>

<template>
  <button
    type="button"
    class="group block w-full rounded-lg p-0 text-left outline-none focus-visible:outline-2 focus-visible:outline-accent"
    @click="$emit('open', album.id)"
  >
    <Artwork
      :hash="album.artwork_hash"
      :radius="8"
      :alt="album.title"
      fluid
      class="transition group-hover:brightness-110"
    />
    <div class="mt-2 truncate font-semibold">{{ album.title }}</div>
    <div class="truncate text-xs text-dim">{{ subtitle }}</div>
  </button>
</template>
