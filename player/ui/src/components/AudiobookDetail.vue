<script setup lang="ts">
import { usePlayToggle } from "../lib/playToggle";
import { BookmarkPlus, Check, Pencil, RotateCcw, Search, Trash2 } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { audioFormat, clock, dayLine, duration, groupByDay, remainingText, SPEEDS, speedLabel } from "../lib/audiobook";
import { useAudiobooksStore } from "../stores/audiobooks";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiSelect from "../ui/UiSelect.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import EditBookDialog from "./EditBookDialog.vue";
import PromptDialog from "../ui/PromptDialog.vue";

const props = defineProps<{ id: number }>();
const books = useAudiobooksStore();

onMounted(() => void books.openDetail(props.id));
watch(() => props.id, (id) => void books.openDetail(id));

const book = computed(() => (books.detail?.id === props.id ? books.detail : null));
const playingThis = computed(() => books.isActive && books.active?.id === props.id);
/** Where the listener is: the live offset while this book plays, else the saved one. */
const offset = computed(() => (playingThis.value ? books.offsetMs : (book.value?.position_ms ?? 0)));
const chapterNow = computed(() => (playingThis.value ? books.chapterIndex : -1));
const days = computed(() => groupByDay(books.history));
const started = computed(() => (book.value?.position_ms ?? 0) > 0);

/** Each chapter's file format ("MP3 · 44.1 kHz · 64 kbps · mono"), from the part it is in. */
const chapterFormats = computed(() => {
  const parts = new Map((book.value?.parts ?? []).map((p) => [p.id, audioFormat(p)]));
  return (book.value?.chapters ?? []).map((c) => parts.get(c.part_id) ?? "");
});

const speedOptions = SPEEDS.map((s) => ({ value: String(s), label: speedLabel(s) }));
const bookSpeed = computed(() => String(playingThis.value ? books.speed : (book.value?.settings.speed ?? 1)));

async function play(from?: number): Promise<void> {
  await books.start(props.id, from);
}
/** Play (or Continue), or Pause while this book plays; paused, it carries on where it is. */
const playButton = usePlayToggle(() => playingThis.value, () => play());

const renaming = ref<{ id: number; name: string } | null>(null);
const editing = ref(false);

const subtitle = computed(() => {
  const b = book.value;
  if (!b) return "";
  const parts: string[] = [];
  if (b.author) parts.push(`by ${b.author}`);
  if (b.narrator) parts.push(`read by ${b.narrator}`);
  return parts.join(" · ");
});
</script>

<template>
  <ViewShell width="medium" section="audiobooks" :crumb="book?.title">
    <StateMessage v-if="books.error && !book" kind="error">{{ books.error }}</StateMessage>
    <StateMessage v-else-if="!book" kind="loading">Loading…</StateMessage>
    <template v-else>
      <div class="detail-hero">
        <Artwork :hash="book.cover_hash" placeholder="book" :size="180" :radius="8" :alt="book.title" />
        <div class="min-w-0 flex-1">
          <h2 class="heading-1 m-0 mb-1" data-testid="book-title">{{ book.title }}</h2>
          <p v-if="subtitle" class="m-0 text-dim" data-testid="book-by">{{ subtitle }}</p>
          <p v-if="book.series" class="m-0 text-xs text-faint">
            {{ book.series }}<template v-if="book.series_index != null"> · book {{ book.series_index }}</template>
            <template v-if="book.year"> · {{ book.year }}</template>
          </p>
          <p class="m-0 mt-2 text-xs text-dim" data-testid="book-length">
            {{ duration(book.duration_ms) }}<template v-if="started && !book.finished_at"> · {{ remainingText(book.duration_ms, offset, books.active?.id === book.id ? books.speed : book.settings.speed) }}</template>
            <template v-if="book.finished_at"> · finished</template>
          </p>
          <div v-if="started" class="mt-2 h-1.5 w-full max-w-[420px] overflow-hidden rounded-full bg-active" data-testid="detail-progress">
            <div class="h-full bg-accent" :style="{ width: `${Math.round(Math.min(1, offset / Math.max(1, book.duration_ms)) * 100)}%` }" />
          </div>
          <div class="mt-4 flex flex-wrap items-center gap-2">
            <UiButton variant="primary" data-testid="play-book" @click="playButton.press()">
              <component :is="playButton.icon" />
              {{ playButton.playing ? "Pause" : started && !book.finished_at ? `Continue from ${clock(offset)}` : "Play" }}
            </UiButton>
            <UiButton v-if="started && !playingThis" data-testid="restart-book" @click="play(0)"><RotateCcw /> Start over</UiButton>
            <UiButton data-testid="toggle-finished" @click="books.setFinished(book.id, !book.finished_at)">
              <Check /> {{ book.finished_at ? "Mark not finished" : "Mark finished" }}
            </UiButton>
            <UiButton data-testid="edit-details" @click="editing = true"><Pencil /> Edit details</UiButton>
            <UiButton v-if="!book.author || !book.year || !book.cover_hash" title="Fill in the missing author, year or cover from Open Library and Google Books" data-testid="look-up" @click="books.lookUpOnline(book.id)">
              <Search /> Look up online
            </UiButton>
          </div>
          <div class="mt-3 flex items-center gap-2 text-xs text-dim">
            Speed
            <UiSelect
              aria-label="Speed for this book"
              trigger-class="w-[90px]"
              :model-value="bookSpeed"
              :options="speedOptions"
              @update:model-value="(v) => books.setSpeed(Number(v), props.id)"
            />
            <span class="text-faint">remembered for this book</span>
          </div>
        </div>
      </div>

      <section v-if="book.chapters.length > 1" class="mt-6" aria-label="Chapters">
        <h3 class="heading-3 mb-2">Chapters</h3>
        <ol class="m-0 list-none p-0" data-testid="chapters">
          <li v-for="(c, i) in book.chapters" :key="c.id">
            <button
              type="button"
              class="flex w-full items-baseline gap-3 rounded-md px-2.5 py-1.5 text-left text-[13px] hover:bg-hover"
              :class="chapterNow === i ? 'bg-active font-semibold text-accent' : ''"
              data-testid="chapter"
              @click="books.playFrom(book.id, c.start_offset_ms)"
            >
              <span class="w-7 shrink-0 text-right tabular-nums text-faint">{{ i + 1 }}</span>
              <span class="min-w-0 flex-1 truncate">{{ c.title }}</span>
              <span v-if="chapterFormats[i]" class="hidden shrink-0 text-xs font-normal tabular-nums text-faint min-[700px]:inline" data-testid="chapter-format">{{ chapterFormats[i] }}</span>
              <span class="w-16 shrink-0 text-right tabular-nums text-xs text-dim">{{ clock(c.start_offset_ms) }}</span>
            </button>
          </li>
        </ol>
      </section>

      <section class="mt-6" aria-label="Bookmarks">
        <div class="mb-2 flex items-center justify-between">
          <h3 class="heading-3 m-0">Bookmarks</h3>
          <UiButton v-if="playingThis" data-testid="add-bookmark" @click="books.addBookmark()"><BookmarkPlus /> Add at {{ clock(books.offsetMs) }}</UiButton>
        </div>
        <p v-if="book.bookmarks.length === 0" class="m-0 text-xs text-faint">
          {{ playingThis ? "No bookmarks yet." : "Bookmarks you add while listening appear here." }}
        </p>
        <ul v-else class="m-0 list-none p-0" data-testid="bookmarks">
          <li v-for="b in book.bookmarks" :key="b.id" class="group flex items-center gap-2 rounded-md px-2.5 py-1.5 hover:bg-hover">
            <button type="button" class="flex min-w-0 flex-1 items-baseline gap-3 text-left text-[13px]" data-testid="bookmark" @click="books.playFrom(book.id, b.book_offset_ms)">
              <span class="w-16 shrink-0 tabular-nums text-xs text-dim">{{ clock(b.book_offset_ms) }}</span>
              <span class="min-w-0 flex-1 truncate">{{ b.name }}</span>
            </button>
            <UiButton variant="icon" title="Rename" aria-label="Rename bookmark" @click="renaming = { id: b.id, name: b.name }"><Pencil /></UiButton>
            <UiButton variant="icon-danger" title="Delete" aria-label="Delete bookmark" data-testid="delete-bookmark" @click="books.removeBookmark(b.id)"><Trash2 /></UiButton>
          </li>
        </ul>
      </section>

      <section class="mt-6" aria-label="Listening history">
        <h3 class="heading-3 mb-2">History</h3>
        <p v-if="days.length === 0" class="m-0 text-xs text-faint">Nothing listened to yet.</p>
        <ul v-else class="m-0 list-none p-0" data-testid="history">
          <li v-for="d in days" :key="d.day" class="flex items-baseline justify-between gap-3 rounded-md px-2.5 py-1.5 text-[13px]">
            <span data-testid="history-day">{{ dayLine(d) }}</span>
            <button type="button" class="shrink-0 text-xs text-accent hover:underline" @click="books.playFrom(book.id, d.stoppedAtMs)">Resume here</button>
          </li>
        </ul>
      </section>

      <EditBookDialog v-model:open="editing" :book="book" @save="(e) => books.editMeta(book!.id, e)" />
      <PromptDialog
        :open="renaming !== null"
        title="Rename bookmark"
        label="Name"
        :initial="renaming?.name ?? ''"
        confirm-label="Save"
        :maxlength="80"
        @update:open="(v) => !v && (renaming = null)"
        @submit="(n) => renaming && books.renameBookmark(renaming.id, n)"
      />
    </template>
  </ViewShell>
</template>
