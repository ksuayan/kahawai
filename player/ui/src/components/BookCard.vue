<script setup lang="ts">
import { Check } from "lucide-vue-next";
import { computed } from "vue";
import { duration } from "../lib/audiobook";
import type { Audiobook } from "../types";
import Artwork from "./Artwork.vue";

/** One book tile: cover, title, author, and how far along you are. */
const props = defineProps<{ book: Audiobook }>();
defineEmits<{ (e: "open", id: number): void }>();

const percent = computed(() => Math.round(props.book.progress * 100));
const started = computed(() => props.book.last_played_at !== null && props.book.progress > 0 && !props.book.finished_at);
const subtitle = computed(() => {
  const b = props.book;
  const parts: string[] = [];
  if (b.author) parts.push(b.author);
  if (b.series) parts.push(b.series_index != null ? `${b.series} #${b.series_index}` : b.series);
  return parts.join(" · ");
});
</script>

<template>
  <button
    type="button"
    class="group block w-full rounded-lg p-0 text-left outline-none focus-visible:outline-2 focus-visible:outline-accent"
    data-testid="book-card"
    @click="$emit('open', book.id)"
  >
    <div class="relative">
      <Artwork :hash="book.cover_hash" :radius="6" :alt="book.title" fluid class="transition group-hover:brightness-110" />
      <span
        v-if="book.finished_at"
        class="absolute right-1.5 top-1.5 flex size-5 items-center justify-center rounded-full bg-accent text-white"
        title="Finished"
        data-testid="book-finished"
      >
        <Check class="size-3.5" />
      </span>
      <div v-else-if="started" class="absolute inset-x-0 bottom-0 h-1 overflow-hidden rounded-b-md bg-black/40" data-testid="book-progress">
        <div class="h-full bg-accent" :style="{ width: `${percent}%` }" />
      </div>
    </div>
    <div class="mt-2 truncate font-semibold">{{ book.title }}</div>
    <div class="truncate text-xs text-dim">{{ subtitle || duration(book.duration_ms) }}</div>
  </button>
</template>
