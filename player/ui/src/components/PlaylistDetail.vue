<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import { isPlayable, type Track } from "../types";
import ConfirmDialog from "../ui/ConfirmDialog.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiInput from "../ui/UiInput.vue";
import ViewShell from "../ui/ViewShell.vue";
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
  <ViewShell>
    <UiButton variant="icon" class="mb-3" @click="nav.go('playlists')">‹ Playlists</UiButton>
    <StateMessage v-if="playlists.loading" kind="loading">Loading playlist…</StateMessage>
    <StateMessage v-else-if="playlists.error" kind="error">{{ playlists.error }}</StateMessage>
    <div v-else-if="playlists.detail">
      <div class="mb-4 flex items-start justify-between gap-3">
        <div>
          <div v-if="renaming" class="flex items-center gap-2">
            <UiInput
              v-model="renameText"
              size="title"
              type="text"
              aria-label="Playlist name"
              maxlength="120"
              @keydown.enter="doRename"
              @keydown.escape="renaming = false"
            />
            <UiButton variant="primary" @click="doRename">Save</UiButton>
            <UiButton @click="renaming = false">Cancel</UiButton>
          </div>
          <div v-else class="flex items-center gap-1">
            <h2 class="m-0 text-xl font-semibold">{{ playlists.detail.playlist.name }}</h2>
            <UiButton variant="icon" title="Rename playlist" aria-label="Rename playlist" @click="startRename">✎</UiButton>
          </div>
          <p class="m-0 mt-1 text-dim">{{ playlists.detail.tracks.length }} tracks</p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <UiButton variant="primary" @click="playAll">▶ Play</UiButton>
          <UiButton @click="addAllToQueue">☰ Add to queue</UiButton>
          <UiButton variant="danger" @click="showDelete = true">Delete</UiButton>
        </div>
      </div>
      <StateMessage v-if="playlists.detail.tracks.length === 0" kind="empty">This playlist is empty.</StateMessage>
      <div v-else class="flex flex-col gap-0.5">
        <TrackRow
          v-for="(t, i) in playlists.detail.tracks"
          :key="t.id"
          :track="t"
          :current="queue.current?.id === t.id"
          @play="playFrom"
        >
          <UiButton variant="icon" title="Move up" aria-label="Move up" :disabled="i === 0 || mutating" @click="moveTrack(i, i - 1)">↑</UiButton>
          <UiButton
            variant="icon"
            title="Move down"
            aria-label="Move down"
            :disabled="i === playlists.detail.tracks.length - 1 || mutating"
            @click="moveTrack(i, i + 1)"
          >
            ↓
          </UiButton>
          <UiButton variant="icon-danger"
            title="Remove from playlist"
            aria-label="Remove from playlist"
            :disabled="mutating"
            @click="removeTrack(t.id)"
          >
            ✕
          </UiButton>
        </TrackRow>
      </div>
    </div>
    <ConfirmDialog
      v-model:open="showDelete"
      title="Delete this playlist?"
      description="The playlist is removed; the tracks stay in your library."
      confirm-label="Delete"
      danger
      @confirm="doDelete"
    />
  </ViewShell>
</template>
