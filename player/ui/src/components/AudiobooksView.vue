<script setup lang="ts">
import { computed, onMounted, watch } from "vue";
import { jobFilesDetail, isJobActive } from "../types";
import { useAudiobooksStore } from "../stores/audiobooks";
import { useJobsStore } from "../stores/jobs";
import { useNavStore } from "../stores/nav";
import StateMessage from "../ui/StateMessage.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import ViewShell from "../ui/ViewShell.vue";
import BookCard from "./BookCard.vue";
import VirtualGrid from "./VirtualGrid.vue";

const books = useAudiobooksStore();
const nav = useNavStore();
const jobs = useJobsStore();

/** An audiobook scan or online lookup that is running: the library fills in as it goes. */
const working = computed(() => jobs.jobs.find((j) => isJobActive(j) && /audiobook/i.test(j.label)) ?? null);
const workingText = computed(() => {
  const j = working.value;
  if (!j) return "";
  const what = j.kind === "enrich_books" ? "Looking up book details" : "Scanning audiobooks";
  const detail = jobFilesDetail(j) ?? (j.progress > 0 ? `${Math.round(j.progress * 100)}%` : "");
  return detail ? `${what} · ${detail}` : `${what}…`;
});

onMounted(() => void books.loadLibrary());
// Typing is debounced a little so a search does not fire per keystroke.
let timer: number | undefined;
watch(
  () => [books.query.q, books.query.author, books.query.series, books.query.finished],
  () => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void books.loadLibrary(), 200);
  },
);

const authorOptions = (): UiSelectOption[] => [{ value: null, label: "All authors" }, ...books.authors.map((a) => ({ value: a, label: a }))];
const seriesOptions = (): UiSelectOption[] => [{ value: null, label: "All series" }, ...books.seriesNames.map((a) => ({ value: a, label: a }))];
const finishedOptions: UiSelectOption[] = [
  { value: "all", label: "All books" },
  { value: "no", label: "Not finished" },
  { value: "yes", label: "Finished" },
];

const filtered = (): boolean => !!(books.query.q || books.query.author || books.query.series || books.query.finished !== "all");
</script>

<template>
  <ViewShell title="Audiobooks" :subtitle="`${books.books.length} ${books.books.length === 1 ? 'book' : 'books'}`" width="full">
    <template #actions>
      <UiInput
        v-model="books.query.q"
        class="w-[220px]"
        type="search"
        placeholder="Title, author, narrator…"
        aria-label="Search audiobooks"
        data-testid="book-search"
      />
      <UiSelect
        aria-label="Author"
        trigger-class="w-[150px]"
        :model-value="books.query.author || null"
        :options="authorOptions()"
        @update:model-value="(v) => (books.query.author = v ?? '')"
      />
      <UiSelect
        v-if="books.seriesNames.length"
        aria-label="Series"
        trigger-class="w-[150px]"
        :model-value="books.query.series || null"
        :options="seriesOptions()"
        @update:model-value="(v) => (books.query.series = v ?? '')"
      />
      <UiSelect
        aria-label="Finished"
        trigger-class="w-[130px]"
        :model-value="books.query.finished"
        :options="finishedOptions"
        @update:model-value="(v) => (books.query.finished = (v ?? 'all') as 'all' | 'yes' | 'no')"
      />
    </template>

    <div
      v-if="working"
      class="mb-4 shrink-0 rounded-md border border-line bg-raised px-3 py-2 text-xs text-dim"
      role="status"
      data-testid="books-working"
    >
      {{ workingText }}
      <div class="mt-1.5 h-1 overflow-hidden rounded-sm bg-active">
        <div class="h-full bg-accent transition-all" :style="{ width: `${Math.round(working.progress * 100)}%` }" />
      </div>
    </div>

    <section v-if="books.continueShelf.length && !filtered()" class="mb-5 shrink-0" aria-label="Continue listening" data-testid="continue-shelf">
      <h3 class="heading-3 mb-2">Continue listening</h3>
      <div class="flex gap-4 overflow-x-auto pb-2">
        <div v-for="b in books.continueShelf.slice(0, 10)" :key="b.id" class="w-[132px] shrink-0">
          <BookCard :book="b" @open="(id) => nav.go('audiobook', id)" />
        </div>
      </div>
    </section>

    <StateMessage v-if="books.loading && !books.books.length" kind="loading">Loading audiobooks…</StateMessage>
    <StateMessage v-else-if="books.error" kind="error">{{ books.error }}</StateMessage>
    <StateMessage v-else-if="books.books.length === 0 && filtered()" kind="empty">No book matches.</StateMessage>
    <StateMessage v-else-if="books.books.length === 0" kind="empty">
      No audiobooks yet. Add a folder of them in Settings, under Audiobooks.
    </StateMessage>
    <VirtualGrid v-else :items="books.books" scroll-key="audiobooks">
      <template #item="{ item }">
        <BookCard :book="item" @open="(id) => nav.go('audiobook', id)" />
      </template>
    </VirtualGrid>
  </ViewShell>
</template>
