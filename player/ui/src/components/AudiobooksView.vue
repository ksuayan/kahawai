<script setup lang="ts">
import { X } from "lucide-vue-next";
import { computed, onMounted, watch } from "vue";
import { jobFilesDetail, isJobActive } from "../types";
import { useAudiobooksStore } from "../stores/audiobooks";
import { useJobsStore } from "../stores/jobs";
import { useNavStore } from "../stores/nav";
import { useViewPrefsStore } from "../stores/viewPrefs";
import { audioFormat, duration } from "../lib/audiobook";
import StateMessage from "../ui/StateMessage.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import BookCard from "./BookCard.vue";
import BookContextMenu from "./BookContextMenu.vue";
import BookDialogs from "./BookDialogs.vue";
import ListToolbar from "./ListToolbar.vue";
import VirtualGrid from "./VirtualGrid.vue";
import VirtualList from "./VirtualList.vue";

const books = useAudiobooksStore();
const nav = useNavStore();
const view = useViewPrefsStore();

/** List rows: 48px cover plus padding. */
const ROW_HEIGHT = 60;

/** Author and series, for a list row. */
function byline(b: { author: string | null; series: string | null; series_index: number | null }): string {
  const parts: string[] = [];
  if (b.author) parts.push(b.author);
  if (b.series) parts.push(b.series_index != null ? `${b.series} #${b.series_index}` : b.series);
  return parts.join(" · ") || "Unknown author";
}
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

onMounted(() => {
  void books.loadLibrary();
  void books.loadListeners().catch(() => undefined);
});
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
        v-if="books.listeners.length > 1"
        aria-label="Listener"
        trigger-class="w-[130px]"
        :model-value="books.listener || 'Default'"
        :options="books.listeners.map((l) => ({ value: l.name, label: l.name }))"
        data-testid="listener-switch"
        @update:model-value="(v) => void books.switchListener(v ?? '')"
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
      <ListToolbar v-model:layout="view.prefs.audiobooksLayout" />
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
        <div v-for="b in books.continueShelf.slice(0, 10)" :key="b.id" class="group/shelf relative w-[132px] shrink-0">
          <BookContextMenu :book="b"><BookCard :book="b" @open="(id) => nav.go('audiobook', id)" /></BookContextMenu>
          <button
            type="button"
            class="absolute left-1.5 top-1.5 flex size-6 items-center justify-center rounded-full bg-black/60 text-white opacity-0 outline-none transition-opacity hover:bg-black/80 focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-accent group-hover/shelf:opacity-100"
            :title="`Remove ${b.title} from Continue listening (your place is kept)`"
            :aria-label="`Remove ${b.title} from Continue listening`"
            data-testid="shelf-dismiss"
            @click="books.dismissFromShelf(b.id)"
          >
            <X class="size-3.5" />
          </button>
        </div>
      </div>
    </section>

    <StateMessage v-if="books.loading && !books.books.length" kind="loading">Loading audiobooks…</StateMessage>
    <StateMessage v-else-if="books.error" kind="error">{{ books.error }}</StateMessage>
    <StateMessage v-else-if="books.books.length === 0 && filtered()" kind="empty">No book matches.</StateMessage>
    <StateMessage v-else-if="books.books.length === 0" kind="empty">
      No audiobooks yet. Add a folder of them in Settings, under Audiobooks.
    </StateMessage>
    <VirtualGrid v-else-if="view.prefs.audiobooksLayout === 'grid'" :items="books.books" scroll-key="audiobooks">
      <template #item="{ item }">
        <BookContextMenu :book="item"><BookCard :book="item" @open="(id) => nav.go('audiobook', id)" /></BookContextMenu>
      </template>
    </VirtualGrid>
    <VirtualList v-else :items="books.books" scroll-key="audiobooks:list" :row-height="ROW_HEIGHT" :get-key="(b) => b.id">
      <template #item="{ item }">
        <BookContextMenu :book="item">
        <button
          type="button"
          class="flex h-full w-full items-center gap-3 rounded-md px-2.5 text-left hover:bg-hover"
          data-testid="book-row"
          @click="nav.go('audiobook', item.id)"
        >
          <Artwork :hash="item.cover_hash" placeholder="book" :size="48" :radius="4" :alt="item.title" />
          <div class="min-w-0 flex-1">
            <div class="truncate font-semibold">{{ item.title }}</div>
            <div class="truncate text-xs text-dim">{{ byline(item) }}</div>
          </div>
          <span class="hidden w-56 shrink-0 truncate text-right text-xs tabular-nums text-faint min-[900px]:block" :title="audioFormat(item)" data-testid="book-row-format">
            {{ audioFormat(item) }}
          </span>
          <span class="w-24 shrink-0 text-right text-xs tabular-nums text-faint" data-testid="book-row-progress">
            {{ item.finished_at ? "Finished" : item.progress > 0 ? `${Math.round(item.progress * 100)}%` : "" }}
          </span>
          <span class="w-20 shrink-0 text-right text-xs tabular-nums text-dim">{{ duration(item.duration_ms) }}</span>
        </button>
        </BookContextMenu>
      </template>
    </VirtualList>
    <BookDialogs />
  </ViewShell>
</template>
