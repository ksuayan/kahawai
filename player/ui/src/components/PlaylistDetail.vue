<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import { isPlayable, type Track } from "../types";
import TrackRow from "./TrackRow.vue";

const props = defineProps<{ id: number }>();

const nav = useNavStore();
const playlists = usePlaylistsStore();
const queue = useQueueStore();
const toasts = useToastsStore();

const renaming = ref(false);
const renameText = ref("");
const showDelete = ref(false);
const mutating = ref(false);

async function load(id: number): Promise<void> {
  await playlists.open(id);
}

onMounted(() => load(props.id));
watch(() => props.id, (id) => load(id));

function playFrom(track: Track): void {
  const list = (playlists.detail?.tracks ?? []).filter(isPlayable);
  const idx = list.findIndex((t) => t.id === track.id);
  if (idx >= 0) void queue.playAll(list, idx);
}

function playAll(): void {
  const list = (playlists.detail?.tracks ?? []).filter(isPlayable);
  if (list.length > 0) void queue.playAll(list, 0);
}

async function addAllToQueue(): Promise<void> {
  const list = (playlists.detail?.tracks ?? []).filter(isPlayable);
  if (list.length === 0) return;
  await queue.appendTracks(list);
  toasts.push("success", `Added ${list.length} tracks to queue`);
}

async function doRename(): Promise<void> {
  const d = playlists.detail;
  const name = renameText.value.trim();
  renaming.value = false;
  if (!d || !name || name === d.playlist.name) return;
  try {
    await playlists.rename(d.playlist.id, name);
  } catch (e) {
    toasts.push("error", "Rename failed", { detail: e instanceof Error ? e.message : String(e) });
  }
}

async function doDelete(): Promise<void> {
  const id = playlists.detail?.playlist.id;
  if (id == null) return;
  await playlists.remove(id);
  nav.go("playlists");
}

async function removeTrack(id: number): Promise<void> {
  mutating.value = true;
  try {
    await playlists.removeTrack(id);
  } catch (e) {
    toasts.push("error", "Remove failed", { detail: e instanceof Error ? e.message : String(e) });
  } finally {
    mutating.value = false;
  }
}

/** Functional reorder: swap with the neighbor and persist via replace. */
async function moveTrack(from: number, to: number): Promise<void> {
  const d = playlists.detail;
  if (!d || to < 0 || to >= d.tracks.length || from === to) return;
  const ids = d.playlist.track_ids.slice();
  const [id] = ids.splice(from, 1);
  ids.splice(to, 0, id);
  mutating.value = true;
  try {
    await playlists.replaceTracks(ids);
  } catch (e) {
    toasts.push("error", "Reorder failed", { detail: e instanceof Error ? e.message : String(e) });
  } finally {
    mutating.value = false;
  }
}

function startRename(): void {
  renameText.value = playlists.detail?.playlist.name ?? "";
  renaming.value = true;
}
</script>

<template>
  <div class="view">
    <button class="back icon-btn" @click="nav.go('playlists')">‹ Playlists</button>
    <div v-if="playlists.loading" class="spinner">Loading playlist…</div>
    <div v-else-if="playlists.error" class="error-banner">{{ playlists.error }}</div>
    <div v-else-if="playlists.detail">
      <div class="header">
        <div class="title-block">
          <template v-if="renaming">
            <input
              v-model="renameText"
              type="text"
              class="rename-input"
              maxlength="120"
              @keydown.enter="doRename"
              @keydown.escape="renaming = false"
            />
            <button class="primary" @click="doRename">Save</button>
            <button @click="renaming = false">Cancel</button>
          </template>
          <template v-else>
            <h2>{{ playlists.detail.playlist.name }}</h2>
            <button class="icon-btn" title="Rename playlist" @click="startRename">✎</button>
          </template>
          <p class="sub">{{ playlists.detail.tracks.length }} tracks</p>
        </div>
        <div class="actions">
          <button class="primary" @click="playAll">▶ Play</button>
          <button @click="addAllToQueue">☰ Add to queue</button>
          <button v-if="!showDelete" class="danger" @click="showDelete = true">Delete</button>
          <span v-else class="confirm">
            Delete this playlist?
            <button class="danger" @click="doDelete">Yes</button>
            <button @click="showDelete = false">No</button>
          </span>
        </div>
      </div>
      <div v-if="playlists.detail.tracks.length === 0" class="empty">This playlist is empty.</div>
      <div v-else class="tracks">
        <div
          v-for="(t, i) in playlists.detail.tracks"
          :key="t.id"
          class="pl-row"
        >
          <TrackRow
            :track="t"
            :current="queue.current?.id === t.id"
            @play="playFrom"
          >
            <button
              class="icon-btn"
              title="Move up"
              :disabled="i === 0 || mutating"
              @click="moveTrack(i, i - 1)"
            >
              ↑
            </button>
            <button
              class="icon-btn"
              title="Move down"
              :disabled="i === playlists.detail.tracks.length - 1 || mutating"
              @click="moveTrack(i, i + 1)"
            >
              ↓
            </button>
            <button
              class="icon-btn danger"
              title="Remove from playlist"
              :disabled="mutating"
              @click="removeTrack(t.id)"
            >
              ✕
            </button>
          </TrackRow>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.back {
  margin-bottom: 12px;
}

.header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  margin-bottom: 16px;
  gap: 12px;
}

.title-block h2 {
  margin: 0 4px 4px 0;
  display: inline;
}

.rename-input {
  font-size: 18px;
  font-weight: 700;
  margin-bottom: 4px;
}

.actions {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-shrink: 0;
}

.confirm {
  display: flex;
  gap: 6px;
  align-items: center;
  color: var(--text-dim);
  font-size: 12px;
}

.tracks {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.pl-row :deep(.track-row) {
  padding-right: 4px;
}
</style>
