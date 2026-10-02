<script setup lang="ts">
import { Pencil, Play } from "lucide-vue-next";
import { computed } from "vue";
import { audioFormat, duration } from "../lib/audiobook";
import { useAudiobooksStore } from "../stores/audiobooks";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import Artwork from "./Artwork.vue";
import EditBookDialog from "./EditBookDialog.vue";

/** The Info and Edit details dialogs a book's menu opens. Mounted once per view. */
const books = useAudiobooksStore();

const book = computed(() => books.bookDialog?.book ?? null);
const infoOpen = computed({
  get: () => books.bookDialog?.kind === "info",
  set: (v: boolean) => {
    if (!v) books.bookDialog = null;
  },
});
const editOpen = computed({
  get: () => books.bookDialog?.kind === "edit",
  set: (v: boolean) => {
    if (!v) books.bookDialog = null;
  },
});

/** Label/value rows; empty values are left out. */
const rows = computed<[string, string][]>(() => {
  const b = book.value;
  if (!b) return [];
  const out: [string, string | null][] = [
    ["Narrator", b.narrator],
    ["Series", b.series ? (b.series_index != null ? `${b.series} #${b.series_index}` : b.series) : null],
    ["Year", b.year != null ? String(b.year) : null],
    ["Length", duration(b.duration_ms)],
    ["Progress", b.finished_at ? "Finished" : b.progress > 0 ? `${Math.round(b.progress * 100)}%` : "Not started"],
    ["Files", `${b.parts.length}`],
    ["Chapters", b.chapters.length ? `${b.chapters.length}` : null],
    ["Format", audioFormat(b) || null],
    ["Folder", b.path],
  ];
  return out.filter((r): r is [string, string] => !!r[1]);
});

function edit(): void {
  if (book.value) books.bookDialog = { kind: "edit", book: book.value };
}
function play(): void {
  const b = book.value;
  books.bookDialog = null;
  if (b) void books.start(b.id);
}
</script>

<template>
  <UiDialog v-model:open="infoOpen" title="Book info" wide>
    <div v-if="book" class="flex flex-col gap-5 sm:flex-row" data-testid="book-info">
      <div class="w-40 shrink-0">
        <Artwork :hash="book.cover_hash" :size="160" :radius="6" :alt="book.title" />
      </div>
      <div class="min-w-0 flex-1">
        <h3 class="heading-2 m-0 mb-1 break-words">{{ book.title }}</h3>
        <p v-if="book.author" class="m-0 mb-4 text-dim">{{ book.author }}</p>
        <dl class="m-0 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-[13px]">
          <template v-for="[label, value] in rows" :key="label">
            <dt class="text-dim">{{ label }}</dt>
            <dd class="m-0" :class="label === 'Folder' && 'select-text break-all font-mono text-xs'" :data-testid="`book-info-${label}`">
              {{ value }}
            </dd>
          </template>
        </dl>
      </div>
    </div>
    <template #footer>
      <UiButton @click="edit"><Pencil /> Edit details</UiButton>
      <UiButton @click="play"><Play /> Play</UiButton>
      <UiButton variant="primary" @click="infoOpen = false">Close</UiButton>
    </template>
  </UiDialog>
  <EditBookDialog v-if="book" v-model:open="editOpen" :book="book" @save="(e) => books.editMeta(book!.id, e)" />
</template>
