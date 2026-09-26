<script setup lang="ts">
import { ref } from "vue";
import { usePlaylistsStore } from "../stores/playlists";
import { usePlayerStore } from "../stores/player";
import { useQueueStore } from "../stores/queue";
import { formatDuration, trackTitle, type Track } from "../types";
import PromptDialog from "../ui/PromptDialog.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";

const queue = useQueueStore();
const player = usePlayerStore();
const playlists = usePlaylistsStore();

const dragged = ref<number | null>(null);
const dropTarget = ref<number | null>(null);
const saving = ref(false);
const saveDialog = ref(false);
const saveError = ref<string | null>(null);

function onDragStart(i: number, e: DragEvent): void {
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

function rowTitle(t: Track): string {
  return `${trackTitle(t)}${t.artist ? ` — ${t.artist}` : ""}`;
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
  <ViewShell title="Queue" :subtitle="`${queue.tracks.length} tracks`">
    <template #actions>
      <UiButton
        variant="icon"
        :pressed="player.shuffle"
        title="Shuffle"
        @click="player.toggleShuffle()"
      >
        🔀 Shuffle
      </UiButton>
      <UiButton
        variant="icon"
        :pressed="player.repeat !== 'off'"
        :title="player.repeat === 'one' ? 'Repeat one' : player.repeat === 'all' ? 'Repeat all' : 'Repeat off'"
        @click="player.cycleRepeat()"
      >
        {{ player.repeat === "one" ? "🔂 Repeat one" : player.repeat === "all" ? "🔁 Repeat all" : "🔁 Repeat off" }}
      </UiButton>
      <UiButton :disabled="queue.tracks.length === 0 || saving" @click="saveDialog = true">
        {{ saving ? "Saving…" : "Save queue as playlist" }}
      </UiButton>
      <UiButton :disabled="queue.tracks.length === 0" @click="queue.clear()">Clear</UiButton>
    </template>

    <StateMessage v-if="saveError" kind="error">{{ saveError }}</StateMessage>
    <StateMessage v-if="queue.tracks.length === 0" kind="empty">
      Queue is empty. Play an album, playlist, or search result to fill it.
    </StateMessage>
    <div v-else class="flex flex-col gap-0.5">
      <div
        v-for="(t, i) in queue.tracks"
        :key="t.id"
        class="flex cursor-grab items-center gap-3 rounded-md px-2.5 py-[7px]"
        :class="[
          i === queue.index ? 'bg-accent/15' : 'hover:bg-hover',
          dragged === i && 'opacity-40',
          dropTarget === i && dragged !== i && 'shadow-[inset_0_2px_0_var(--color-accent)]',
        ]"
        :data-current="i === queue.index || undefined"
        data-testid="queue-row"
        :title="rowTitle(t)"
        draggable="true"
        @dragstart="onDragStart(i, $event)"
        @dragover="onDragOver(i, $event)"
        @dragleave="onDragLeave"
        @drop="onDrop(i, $event)"
        @dragend="onDragEnd"
      >
        <span class="shrink-0 cursor-grab tracking-[-2px] text-faint" aria-hidden="true">⋮⋮</span>
        <span class="w-7 shrink-0 text-right tabular-nums text-faint">{{ i + 1 }}</span>
        <Artwork :hash="null" :size="32" :radius="4" />
        <div class="min-w-0 flex-1">
          <div class="truncate">{{ trackTitle(t) }}</div>
          <div v-if="t.artist" class="truncate text-xs text-dim">{{ t.artist }}</div>
        </div>
        <span class="shrink-0 tabular-nums text-dim">{{ formatDuration(t.duration_ms) }}</span>
        <span class="flex shrink-0 gap-0.5">
          <UiButton variant="icon" title="Move up" aria-label="Move up" :disabled="i === 0" @click="queue.moveUp(i)">▲</UiButton>
          <UiButton
            variant="icon"
            title="Move down"
            aria-label="Move down"
            :disabled="i === queue.tracks.length - 1"
            @click="queue.moveDown(i)"
          >
            ▼
          </UiButton>
          <UiButton variant="icon-danger"
            title="Remove from queue"
            aria-label="Remove from queue"
            @click="queue.removeAt(i)"
          >
            ✕
          </UiButton>
        </span>
      </div>
    </div>

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
