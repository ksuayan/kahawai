<script setup lang="ts">
import { computed } from "vue";
import { sanitizeNotes } from "../lib/podcast";
import { openUrl } from "../tauri";

/**
 * An episode's show notes: the feed's HTML made safe (lib/podcast's
 * sanitizeNotes: formatting, lists and web links only), with links opening in
 * the browser, never inside the app.
 */
const props = defineProps<{ html: string | null | undefined }>();
const safe = computed(() => sanitizeNotes(props.html));

function onClick(e: MouseEvent): void {
  const a = (e.target as HTMLElement | null)?.closest("a");
  if (!a) return;
  e.preventDefault();
  const href = a.getAttribute("href");
  if (href && a.hasAttribute("data-external")) void openUrl(href);
}
</script>

<template>
  <!-- Trusted after sanitizing: tags and attributes are allow-listed. -->
  <div v-if="safe" class="show-notes select-text" data-testid="show-notes" @click="onClick" v-html="safe" />
  <p v-else class="m-0 text-dim" data-testid="show-notes-empty">No show notes.</p>
</template>

<style scoped>
.show-notes {
  font-family: var(--font-serif);
  font-size: 15px;
  line-height: 1.6;
  color: var(--color-fg);
  max-width: 70ch;
  overflow-wrap: anywhere;
}
.show-notes :deep(p) {
  margin: 0.5rem 0;
}
.show-notes :deep(ul),
.show-notes :deep(ol) {
  margin: 0.5rem 0;
  padding-left: 1.25rem;
}
.show-notes :deep(ul) {
  list-style: disc;
}
.show-notes :deep(ol) {
  list-style: decimal;
}
.show-notes :deep(a[href]) {
  color: var(--color-accent);
  text-decoration: underline;
  cursor: pointer;
}
.show-notes :deep(h1),
.show-notes :deep(h2),
.show-notes :deep(h3),
.show-notes :deep(h4) {
  font-family: var(--font-sans);
  font-size: 15px;
  font-weight: 600;
  margin: 1rem 0 0.25rem;
}
.show-notes :deep(blockquote) {
  margin: 0.5rem 0;
  padding-left: 0.75rem;
  border-left: 2px solid var(--color-line);
  color: var(--color-dim);
}
</style>
