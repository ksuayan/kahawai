<script setup lang="ts">
import { TriangleAlert } from "lucide-vue-next";
import type { PodcastFeed } from "../types";
import Artwork from "./Artwork.vue";

/** One show tile: artwork, title, author, unplayed count, and a failing-feed badge. */
defineProps<{ feed: PodcastFeed }>();
defineEmits<{ (e: "open", id: number): void }>();
</script>

<template>
  <button
    type="button"
    class="group block w-full rounded-lg p-0 text-left outline-none focus-visible:outline-2 focus-visible:outline-accent"
    data-testid="podcast-card"
    @click="$emit('open', feed.id)"
  >
    <div class="relative">
      <Artwork :url="feed.image_url" placeholder="podcast" :radius="6" :alt="feed.title" fluid class="transition group-hover:brightness-110" />
      <span
        v-if="feed.unplayed_count > 0"
        class="absolute right-1.5 top-1.5 min-w-6 rounded-full bg-accent px-1.5 text-center text-[12px] font-semibold leading-6 text-white"
        :title="`${feed.unplayed_count} unplayed`"
        data-testid="podcast-unplayed"
      >
        {{ feed.unplayed_count }}
      </span>
      <span
        v-if="feed.last_error"
        class="absolute bottom-1.5 left-1.5 flex size-6 items-center justify-center rounded-full bg-danger text-white"
        :title="`The feed couldn't be read: ${feed.last_error}`"
        data-testid="podcast-failing"
      >
        <TriangleAlert class="size-3.5" />
      </span>
    </div>
    <div class="mt-2 truncate font-semibold">{{ feed.title }}</div>
    <div class="truncate text-xs text-dim">{{ feed.author || `${feed.episode_count} episodes` }}</div>
  </button>
</template>
