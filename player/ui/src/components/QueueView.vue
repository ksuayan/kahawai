<script setup lang="ts">
import { ChevronDown, ChevronUp, GripVertical, Repeat, Repeat1, Shuffle, X } from "lucide-vue-next";
import { computed, ref } from "vue";
import { sortTracks, trackSortOptions } from "../lib/sorting";
import { useLibraryStore } from "../stores/library";
import { usePlaylistsStore } from "../stores/playlists";
import { usePlayerStore } from "../stores/player";
import { useQueueStore } from "../stores/queue";
import { useViewPrefsStore } from "../stores/viewPrefs";
import {
  formatBadge,
  formatDuration,
  isPlayable,
  mqaLabel,
  mqaTitle,
  qualityTitle,
  trackTitle,
  unplayableReason,
  type Track,
} from "../types";
import UiBadge from "../ui/UiBadge.vue";
import PromptDialog from "../ui/PromptDialog.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import ListToolbar from "./ListToolbar.vue";
import TrackCollection from "./TrackCollection.vue";

const queue = useQueueStore();
const player = usePlayerStore();
const playlists = usePlaylistsStore();
const lib = useLibraryStore();
const view = useViewPrefsStore();
const SORTS = trackSortOptions("Queue order");

/** A queue entry and its position: sorting only changes what's shown, never
 *  the play order, so every action goes through `i`. */
interface Entry {
  t: Track;
  i: number;
}
const entries = computed<Entry[]>(() => queue.tracks.map((t, i) => ({ t, i })));
const shown = computed(() => sortTracks(entries.value, view.prefs.queueSort, (e) => e.t));
/** Shown in another order than the play order: rearranging is off. */
const sorted = computed(() => view.prefs.queueSort !== "default");

const dragged = ref<number | null>(null);
const dropTarget = ref<number | null>(null);
const saving = ref(false);
const saveDialog = ref(false);
const saveError = ref<string | null>(null);

function onDragStart(i: number, e: DragEvent): void {
  if (sorted.value) return;
  dragged.value = i;
  dropTarget.value = null;
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", String(i));
  }
}

function onDragOver(i: number, e: DragEvent): void {
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
  dropTarget.value = i;
}

function onDragLeave(): void {
  dropTarget.value = null;
}

async function onDrop(i: number, e: DragEvent): Promise<void> {
  e.preventDefault();
  const raw = e.dataTransfer?.getData("text/plain");
  const from = dragged.value ?? (raw ? Number(raw) : NaN);
  dragged.value = null;
  dropTarget.value = null;
  if (Number.isInteger(from) && from >= 0 && from !== i) await queue.reorder(from, i);
}

function onDragEnd(): void {
  dragged.value = null;
  dropTarget.value = null;
}

/** Cover of the track's album (the queue holds tracks, not albums). */
function coverOf(t: Track): string | null {
  return lib.artworkFor(t);
}

function detailLine(t: Track): string {
  return [t.artist, t.album].filter(Boolean).join(" — ");
}

function rowTitle(t: Track): string {
  const base = `${trackTitle(t)}${t.artist ? ` — ${t.artist}` : ""}`;
  return isPlayable(t) ? `${base}\nDouble-click to play` : `${base} — ${unplayableReason(t)}`;
}

/** Double-click / Enter: play the queue from this row. */
function playRow(i: number): void {
  const t = queue.tracks[i];
  if (t && isPlayable(t)) void queue.playAt(i);
}

async function saveAsPlaylist(name: string): Promise<void> {
  saving.value = true;
  saveError.value = null;
  try {
    await playlists.saveQueueAs(name);
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <ViewShell title="Queue" :subtitle="`${queue.tracks.length} tracks`" width="full">
    <template #actions>
      <UiButton
        variant="icon"
        :pressed="player.shuffle"
        title="Shuffle"
        @click="player.toggleShuffle()"
      >
        <Shuffle /> Shuffle
      </UiButton>
      <UiButton
        variant="icon"
        :pressed="player.repeat !== 'off'"
        :title="player.repeat === 'one' ? 'Repeat one' : player.repeat === 'all' ? 'Repeat all' : 'Repeat off'"
        @click="player.cycleRepeat()"
      >
        <Repeat1 v-if="player.repeat === 'one'" />
        <Repeat v-else />
        {{ player.repeat === "one" ? "Repeat one" : player.repeat === "all" ? "Repeat all" : "Repeat off" }}
      </UiButton>
      <UiButton :disabled="queue.tracks.length === 0 || saving" @click="saveDialog = true">
        {{ saving ? "Saving…" : "Save queue as playlist" }}
      </UiButton>
      <UiButton :disabled="queue.tracks.length === 0" @click="queue.clear()">Clear</UiButton>
    </template>

    <div class="shrink-0">
      <StateMessage v-if="saveError" kind="error">{{ saveError }}</StateMessage>
      <div v-if="queue.tracks.length > 0" class="mb-3 flex flex-wrap items-center justify-between gap-3">
        <p class="m-0 text-xs" :class="sorted ? 'text-dim' : 'text-faint'" data-testid="queue-sort-hint">
          {{
            sorted
              ? "Sorted for viewing: the play order is unchanged. Choose Queue order to rearrange."
              : "Drag rows, or use the arrows, to change the play order."
          }}
        </p>
        <ListToolbar v-model:layout="view.prefs.queueLayout" v-model:sort="view.prefs.queueSort" :sort-options="SORTS" />
      </div>
    </div>
    <StateMessage v-if="queue.tracks.length === 0" kind="empty">
      Queue is empty. Play an album, playlist, or search result to fill it.
    </StateMessage>
    <TrackCollection
      v-else
      :items="shown"
      :track="(e) => e.t"
      :layout="view.prefs.queueLayout"
      scroll-key="queue"
      :get-key="(e) => `${e.i}:${e.t.id}`"
      :is-current="(e) => e.i === queue.index"
      :number="(e) => e.i + 1"
      @play="(e) => playRow(e.i)"
    >
      <template #row="{ item: { t, i } }">
        <div
          class="flex cursor-default items-center gap-3 rounded-md px-2.5 py-[7px] outline-none focus-visible:outline-2 focus-visible:outline-accent"
          :class="[
            i === queue.index ? 'bg-accent/15' : 'hover:bg-hover',
            !isPlayable(t) && 'opacity-45',
            dragged === i && 'opacity-40',
            dropTarget === i && dragged !== i && 'shadow-[inset_0_2px_0_var(--color-accent)]',
          ]"
          :data-current="i === queue.index || undefined"
          :data-playable="isPlayable(t)"
          data-testid="queue-row"
          tabindex="0"
          :title="rowTitle(t)"
          :draggable="!sorted"
          @dblclick="playRow(i)"
          @keydown.enter.self="playRow(i)"
          @dragstart="onDragStart(i, $event)"
          @dragover="onDragOver(i, $event)"
          @dragleave="onDragLeave"
          @drop="onDrop(i, $event)"
          @dragend="onDragEnd"
        >
          <GripVertical
            class="size-4 shrink-0 text-faint"
            :class="sorted ? 'opacity-30' : 'cursor-grab'"
            aria-hidden="true"
          />
          <span class="w-7 shrink-0 text-right tabular-nums text-faint">{{ i + 1 }}</span>
          <Artwork :hash="coverOf(t)" :size="36" :radius="4" />
          <div class="min-w-0 flex-1">
            <div class="truncate">{{ trackTitle(t) }}</div>
            <div v-if="detailLine(t)" class="truncate text-xs text-dim" data-testid="detail-line">{{ detailLine(t) }}</div>
          </div>
          <UiBadge :title="qualityTitle(t)" data-testid="format-badge">{{ formatBadge(t) }}</UiBadge>
          <UiBadge v-if="t.mqa" variant="accent" :title="mqaTitle(t)" data-testid="mqa-badge">{{ mqaLabel(t) }}</UiBadge>
          <span class="shrink-0 tabular-nums text-dim">{{ formatDuration(t.duration_ms) }}</span>
          <span class="flex shrink-0 gap-0.5">
            <UiButton
              variant="icon"
              title="Move up"
              aria-label="Move up"
              :disabled="sorted || i === 0"
              @click="queue.moveUp(i)"
            >
              <ChevronUp />
            </UiButton>
            <UiButton
              variant="icon"
              title="Move down"
              aria-label="Move down"
              :disabled="sorted || i === queue.tracks.length - 1"
              @click="queue.moveDown(i)"
            >
              <ChevronDown />
            </UiButton>
            <UiButton variant="icon-danger" title="Remove from queue" aria-label="Remove from queue" @click="queue.removeAt(i)">
              <X />
            </UiButton>
          </span>
        </div>
      </template>
    </TrackCollection>

    <PromptDialog
      v-model:open="saveDialog"
      title="Save queue as playlist"
      label="Playlist name"
      placeholder="Playlist name"
      confirm-label="Save"
      @submit="saveAsPlaylist"
    />
  </ViewShell>
</template>
