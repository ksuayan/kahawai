<script setup lang="ts">
import { ChevronDown, ChevronUp, GripVertical, Repeat, Repeat1, Shuffle, X } from "lucide-vue-next";
import { computed, onBeforeUnmount, ref } from "vue";
import { useBreakpoint } from "../lib/breakpoint";
import { isNoopSlot, partingFor, slotFromY, targetIndex } from "../lib/listdrop";
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
import ItemContextMenu from "./ItemContextMenu.vue";
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

const saving = ref(false);
const saveDialog = ref(false);
const saveError = ref<string | null>(null);

// ------------------------------------------------------------ drag & drop ----
// Pointer-based, like Koa's photo grid, NOT the browser's native drag-and-drop:
// in the webview the native drag image of a virtualized (transformed) row is a
// faint copy of the wrong row sliding in from the top, and it can't be styled.
// Here the pointer drives it all: a copy of the row follows the pointer, the
// row stays put dimmed inside a dashed outline, and the gap where it will land
// opens up and is highlighted. Esc cancels; near the edges the list scrolls.

/** Row height in list mode; the drop geometry is computed from it. */
const { isPhone } = useBreakpoint();
const ROW_HEIGHT = 52;
/** Pixels the pointer must travel before a press becomes a drag (so clicks still click). */
const DRAG_THRESHOLD = 5;
/** How far the two rows beside the open slot step apart (px). */
const PART = 4;

/** The queue position being dragged. */
const dragFrom = ref<number | null>(null);
/** The insertion slot under the pointer (0..n), when dropping there would change the order. */
const slot = ref<number | null>(null);
let armed: { i: number; x: number; y: number; row: HTMLElement } | null = null;
let ghost: HTMLElement | null = null;
let grab = { x: 0, y: 0 };
let pointer = { x: 0, y: 0 };
let rowsEl: HTMLElement | null = null;
let scrollEl: HTMLElement | null = null;

function onRowPointerDown(e: PointerEvent, i: number): void {
  if (sorted.value || e.button !== 0) return;
  if ((e.target as HTMLElement | null)?.closest("button")) return; // move / remove buttons
  // Touch: only the grip handle starts a drag, so vertical scrolling still works.
  const fromHandle = !!(e.target as HTMLElement | null)?.closest("[data-drag-handle]");
  if (e.pointerType === "touch" && !fromHandle) return;
  armed = { i, x: e.clientX, y: e.clientY, row: e.currentTarget as HTMLElement };
  pointer = { x: e.clientX, y: e.clientY };
  window.addEventListener("pointermove", onPointerMove, true);
  window.addEventListener("pointerup", onPointerUp, true);
  window.addEventListener("pointercancel", finishDrag, true);
  window.addEventListener("keydown", onDragKey, true);
}

function detach(): void {
  window.removeEventListener("pointermove", onPointerMove, true);
  window.removeEventListener("pointerup", onPointerUp, true);
  window.removeEventListener("pointercancel", finishDrag, true);
  window.removeEventListener("keydown", onDragKey, true);
}

function onPointerMove(e: PointerEvent): void {
  pointer = { x: e.clientX, y: e.clientY };
  if (dragFrom.value === null) {
    if (!armed || Math.hypot(e.clientX - armed.x, e.clientY - armed.y) < DRAG_THRESHOLD) return;
    beginDrag();
  }
  moveGhost();
  updateSlot();
  autoScrollFor(e.clientY);
  e.preventDefault();
}

function beginDrag(): void {
  if (!armed) return;
  dragFrom.value = armed.i;
  slot.value = null;
  rowsEl = armed.row.closest<HTMLElement>("[data-rows]");
  scrollEl = armed.row.closest<HTMLElement>("[data-scroller]");
  makeGhost(armed.row);
  document.body.style.cursor = "grabbing";
  document.body.style.userSelect = "none";
  window.getSelection()?.removeAllRanges();
}

/** A copy of the grabbed row that follows the pointer. */
function makeGhost(row: HTMLElement): void {
  const rect = row.getBoundingClientRect();
  const copy = row.cloneNode(true) as HTMLElement;
  copy.removeAttribute("data-testid");
  copy.removeAttribute("title");
  copy.setAttribute("data-testid", "drag-ghost");
  copy.style.cssText +=
    ";position:fixed;left:0;top:0;margin:0;z-index:80;pointer-events:none;opacity:.94;" +
    "background:var(--color-surface);box-shadow:var(--shadow-float);border-radius:8px;" +
    `width:${rect.width}px;height:${rect.height}px;will-change:transform;`;
  grab = { x: pointer.x - rect.left, y: pointer.y - rect.top };
  document.body.appendChild(copy);
  ghost = copy;
}

function moveGhost(): void {
  if (ghost) ghost.style.transform = `translate(${pointer.x - grab.x}px, ${pointer.y - grab.y}px) scale(1.02)`;
}

/** The slot under the pointer, if dropping there would move the row; none outside the list. */
function updateSlot(): void {
  const from = dragFrom.value;
  const box = scrollEl?.getBoundingClientRect();
  const rows = rowsEl?.getBoundingClientRect();
  if (from === null || !box || !rows) return;
  const { x, y } = pointer;
  if (x < box.left || x > box.right || y < box.top || y > box.bottom) {
    slot.value = null;
    return;
  }
  const s = slotFromY(y - rows.top, ROW_HEIGHT, queue.tracks.length);
  slot.value = s === null || isNoopSlot(from, s) ? null : s;
}

async function onPointerUp(e: PointerEvent): Promise<void> {
  pointer = { x: e.clientX, y: e.clientY };
  const from = dragFrom.value;
  if (from === null) {
    detach(); // a plain click: leave it to the row
    armed = null;
    return;
  }
  updateSlot(); // where it was let go
  const s = slot.value;
  finishDrag();
  swallowNextClick(); // the release must not also click the row under it
  if (s !== null) await queue.reorder(from, targetIndex(from, s));
}

/** Escape while holding a row cancels: nothing moves. */
function onDragKey(e: KeyboardEvent): void {
  if (e.key !== "Escape") return;
  if (dragFrom.value !== null) {
    e.stopPropagation();
    e.preventDefault();
  }
  finishDrag();
}

function finishDrag(): void {
  detach();
  armed = null;
  dragFrom.value = null;
  slot.value = null;
  stopAutoScroll();
  ghost?.remove();
  ghost = null;
  rowsEl = scrollEl = null;
  document.body.style.cursor = "";
  document.body.style.userSelect = "";
}

function swallowNextClick(): void {
  const h = (ev: MouseEvent) => {
    ev.stopPropagation();
    ev.preventDefault();
  };
  window.addEventListener("click", h, { capture: true, once: true });
  setTimeout(() => window.removeEventListener("click", h, true), 0);
}

// Edge auto-scroll: dragging near the top or bottom of a long queue scrolls it.
let scrollTimer: ReturnType<typeof setInterval> | null = null;
let scrollVelocity = 0;
const EDGE = 48;

function autoScrollFor(clientY: number): void {
  const r = scrollEl?.getBoundingClientRect();
  if (!r) return;
  if (clientY < r.top + EDGE) scrollVelocity = -Math.ceil(((r.top + EDGE - clientY) / EDGE) * 16);
  else if (clientY > r.bottom - EDGE) scrollVelocity = Math.ceil(((clientY - (r.bottom - EDGE)) / EDGE) * 16);
  else scrollVelocity = 0;
  if (scrollVelocity !== 0 && !scrollTimer) {
    scrollTimer = setInterval(() => {
      if (scrollVelocity === 0) return stopAutoScroll();
      scrollEl?.scrollBy(0, scrollVelocity);
      updateSlot(); // the list moved under a still pointer
    }, 16);
  } else if (scrollVelocity === 0) {
    stopAutoScroll();
  }
}

function stopAutoScroll(): void {
  if (scrollTimer) clearInterval(scrollTimer);
  scrollTimer = null;
  scrollVelocity = 0;
}

onBeforeUnmount(finishDrag);

/** The two rows beside the open slot step apart to show the gap. */
function rowShift(i: number): string | undefined {
  const side = partingFor(i, slot.value);
  return side ? `translateY(${side * PART}px)` : undefined;
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
      :row-height="ROW_HEIGHT"
      @play="(e) => playRow(e.i)"
    >
      <template #row="{ item: { t, i } }">
        <ItemContextMenu :track="t" @play="playRow(i)">
        <div
          class="flex cursor-default items-center gap-3 rounded-md px-2.5 py-[7px] outline-none transition-transform duration-100 ease-out focus-visible:outline-2 focus-visible:outline-accent"
          :class="[
            i === queue.index ? 'bg-accent/15' : dragFrom === null && 'hover:bg-hover',
            !isPlayable(t) && 'opacity-45',
            dragFrom === i && 'opacity-40',
            isPhone && 'min-h-[52px] py-3',
          ]"
          :style="{ transform: rowShift(i) }"
          :data-dragging="dragFrom === i || undefined"
          :data-reorderable="String(!sorted)"
          :data-current="i === queue.index || undefined"
          :data-playable="isPlayable(t)"
          data-testid="queue-row"
          tabindex="0"
          :title="rowTitle(t)"
          @pointerdown="onRowPointerDown($event, i)"
          @dblclick="playRow(i)"
          @keydown.enter.self="playRow(i)"
        >
          <span
            data-drag-handle
            class="flex shrink-0 items-center justify-center rounded"
            :class="isPhone ? '-m-2 size-11' : 'size-4'"
            :style="isPhone ? { touchAction: 'none' } : undefined"
            aria-hidden="true"
          >
            <GripVertical
              class="size-4 text-faint"
              :class="sorted ? 'opacity-30' : 'cursor-grab'"
            />
          </span>
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
        </ItemContextMenu>
      </template>
      <template #overlay>
        <!-- The row being dragged: a dashed outline where it is now. -->
        <div
          v-if="dragFrom !== null"
          class="pointer-events-none absolute inset-x-0 z-10 rounded-lg border-2 border-dashed border-dim/70"
          :style="{ top: `${dragFrom * ROW_HEIGHT + 1}px`, height: `${ROW_HEIGHT - 2}px` }"
          data-testid="drag-outline"
        />
        <!-- Where it will land: a tinted, dashed gap with a bright line in the middle. -->
        <div
          v-if="slot !== null"
          class="pointer-events-none absolute inset-x-1 z-10 flex flex-col justify-center rounded border border-dashed border-accent/70 bg-accent/15 transition-all duration-100 ease-out"
          :style="{ top: `${slot * ROW_HEIGHT - PART - 1}px`, height: `${2 * PART + 2}px` }"
          data-testid="drop-slot"
          :data-slot="slot"
        >
          <div class="mx-1 h-0.5 rounded-full bg-accent" />
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
