<script setup lang="ts">
import { ref } from "vue";
import { usePlaylistsStore } from "../stores/playlists";
import { usePlayerStore } from "../stores/player";
import { useQueueStore } from "../stores/queue";
import { formatDuration, trackTitle, type Track } from "../types";
import Artwork from "./Artwork.vue";

const queue = useQueueStore();
const player = usePlayerStore();
const playlists = usePlaylistsStore();

const dragged = ref<number | null>(null);
const dropTarget = ref<number | null>(null);
const saving = ref(false);
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

async function saveAsPlaylist(): Promise<void> {
  const name = window.prompt("Playlist name:");
  if (!name || !name.trim()) return;
  saving.value = true;
  saveError.value = null;
  try {
    await playlists.saveQueueAs(name.trim());
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="view">
    <div class="header">
      <div>
        <h2>Queue</h2>
        <p class="sub">{{ queue.tracks.length }} tracks</p>
      </div>
      <div class="actions">
        <button
          class="icon-btn mode"
          :class="{ on: player.shuffle }"
          title="Shuffle"
          @click="player.toggleShuffle()"
        >
          🔀 Shuffle
        </button>
        <button
          class="icon-btn mode"
          :class="{ on: player.repeat !== 'off' }"
          :title="player.repeat === 'one' ? 'Repeat one' : player.repeat === 'all' ? 'Repeat all' : 'Repeat off'"
          @click="player.cycleRepeat()"
        >
          {{ player.repeat === "one" ? "🔂 Repeat one" : player.repeat === "all" ? "🔁 Repeat all" : "🔁 Repeat off" }}
        </button>
        <button :disabled="queue.tracks.length === 0 || saving" @click="saveAsPlaylist">
          {{ saving ? "Saving…" : "Save queue as playlist" }}
        </button>
        <button :disabled="queue.tracks.length === 0" @click="queue.clear()">Clear</button>
      </div>
    </div>
    <div v-if="saveError" class="error-banner">{{ saveError }}</div>
    <div v-if="queue.tracks.length === 0" class="empty">
      Queue is empty. Play an album, playlist, or search result to fill it.
    </div>
    <div v-else class="rows">
      <div
        v-for="(t, i) in queue.tracks"
        :key="t.id"
        class="track-row qrow"
        :class="{
          current: i === queue.index,
          dragging: dragged === i,
          'drop-target': dropTarget === i && dragged !== i,
        }"
        :title="rowTitle(t)"
        draggable="true"
        @dragstart="onDragStart(i, $event)"
        @dragover="onDragOver(i, $event)"
        @dragleave="onDragLeave"
        @drop="onDrop(i, $event)"
        @dragend="onDragEnd"
      >
        <span class="grip" aria-hidden="true">⋮⋮</span>
        <span class="num">{{ i + 1 }}</span>
        <Artwork :hash="null" :size="32" :radius="4" />
        <div class="main">
          <div class="title">{{ trackTitle(t) }}</div>
          <div v-if="t.artist" class="artist-line">{{ t.artist }}</div>
        </div>
        <span class="dur">{{ formatDuration(t.duration_ms) }}</span>
        <span class="row-actions">
          <button class="icon-btn" title="Move up" :disabled="i === 0" @click="queue.moveUp(i)">▲</button>
          <button
            class="icon-btn"
            title="Move down"
            :disabled="i === queue.tracks.length - 1"
            @click="queue.moveDown(i)"
          >
            ▼
          </button>
          <button class="icon-btn danger" title="Remove from queue" @click="queue.removeAt(i)">✕</button>
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  margin-bottom: 16px;
}

.header h2 {
  margin: 0 0 4px;
}

.actions {
  display: flex;
  gap: 8px;
  align-items: center;
}

.mode.on {
  color: #0a84ff;
  background: rgba(10, 132, 255, 0.14);
}

.rows {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.qrow {
  cursor: grab;
}

.qrow.dragging {
  opacity: 0.4;
}

.qrow.drop-target {
  box-shadow: inset 0 2px 0 var(--accent);
}

.grip {
  color: var(--text-faint);
  letter-spacing: -2px;
  cursor: grab;
  flex-shrink: 0;
}

.row-actions {
  display: flex;
  gap: 2px;
  flex-shrink: 0;
}

.danger {
  color: var(--text-dim);
}

.danger:hover {
  color: var(--danger);
}
</style>
