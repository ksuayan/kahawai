<script setup lang="ts">
import { usePlayToggle } from "../lib/playToggle";
import { ChevronDown, ChevronUp, GripVertical, ListPlus, Pencil, X } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { useRowDrag } from "../lib/rowDrag";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import { useViewPrefsStore } from "../stores/viewPrefs";
import { isPlayable, type Track } from "../types";
import ConfirmDialog from "../ui/ConfirmDialog.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiInput from "../ui/UiInput.vue";
import ViewShell from "../ui/ViewShell.vue";
import ListToolbar from "./ListToolbar.vue";
import RowDragOverlay from "./RowDragOverlay.vue";
import TrackCollection from "./TrackCollection.vue";
import TrackRow from "./TrackRow.vue";

const props = defineProps<{ id: number }>();

const nav = useNavStore();
const playlists = usePlaylistsStore();
const queue = useQueueStore();
const toasts = useToastsStore();
const view = useViewPrefsStore();

/** List row height; the drag geometry is computed from it. */
const ROW_HEIGHT = 52;
/** A track and its place: a playlist may hold the same track twice, so everything goes by place. */
const entries = computed(() => (playlists.detail?.tracks ?? []).map((t, i) => ({ t, i })));

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
/** Play, or Pause while one of this playlist's tracks plays. */
const playButton = usePlayToggle((t) => (playlists.detail?.tracks ?? []).some((x) => x.id === t.id), playAll);

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

// Drag to reorder in the list (lib/rowDrag, the same as the Queue).
const { dragFrom, slot, onRowPointerDown, rowShift } = useRowDrag({
  rowHeight: ROW_HEIGHT,
  enabled: () => !mutating.value,
  count: () => entries.value.length,
  onMove: (from, to) => moveTrack(from, to),
});

function startRename(): void {
  renameText.value = playlists.detail?.playlist.name ?? "";
  renaming.value = true;
}
</script>

<template>
  <ViewShell width="full" section="playlists" :crumb="playlists.detail?.playlist.name">
    <StateMessage v-if="playlists.loading" kind="loading">Loading playlist…</StateMessage>
    <StateMessage v-else-if="playlists.error" kind="error">{{ playlists.error }}</StateMessage>
    <template v-else-if="playlists.detail">
      <div class="mb-4 flex shrink-0 items-start justify-between gap-3">
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
            <h2 class="heading-1 m-0">{{ playlists.detail.playlist.name }}</h2>
            <UiButton variant="icon" title="Rename playlist" aria-label="Rename playlist" @click="startRename"><Pencil /></UiButton>
          </div>
          <p class="m-0 mt-1 text-dim">{{ playlists.detail.tracks.length }} tracks</p>
        </div>
        <div class="flex shrink-0 flex-wrap items-center justify-end gap-2">
          <UiButton variant="primary" data-testid="play-all" @click="playButton.press()"><component :is="playButton.icon" class="fill-current" /> {{ playButton.label }}</UiButton>
          <UiButton @click="addAllToQueue"><ListPlus /> Add to queue</UiButton>
          <UiButton variant="danger" @click="showDelete = true">Delete</UiButton>
          <ListToolbar v-model:layout="view.prefs.playlistTracksLayout" />
        </div>
      </div>
      <p v-if="playlists.detail.tracks.length > 1 && view.prefs.playlistTracksLayout === 'list'" class="m-0 mb-2 shrink-0 text-xs text-faint" data-testid="playlist-drag-hint">
        Drag rows, or use the arrows, to change the order.
      </p>
      <StateMessage v-if="playlists.detail.tracks.length === 0" kind="empty">This playlist is empty.</StateMessage>
      <TrackCollection
        v-else
        :items="entries"
        :track="(e) => e.t"
        :layout="view.prefs.playlistTracksLayout"
        :scroll-key="`playlist:${playlists.detail.playlist.id}`"
        :get-key="(e) => `${e.i}:${e.t.id}`"
        :is-current="(e) => queue.current?.id === e.t.id"
        :number="(e) => e.i + 1"
        :row-height="ROW_HEIGHT"
        @play="(e) => playFrom(e.t)"
      >
        <template #row="{ item: { t, i } }">
          <div
            class="flex h-full items-center transition-transform duration-100 ease-out"
            :class="dragFrom === i && 'opacity-40'"
            :style="{ transform: rowShift(i) }"
            :data-dragging="dragFrom === i || undefined"
            data-testid="playlist-row"
            @pointerdown="onRowPointerDown($event, i)"
          >
            <GripVertical class="mr-1 size-4 shrink-0 cursor-grab text-faint" aria-hidden="true" />
            <TrackRow class="min-w-0 flex-1" :track="t" :number="i + 1" :current="queue.current?.id === t.id" @play="playFrom">
              <UiButton variant="icon" title="Move up" aria-label="Move up" :disabled="i === 0 || mutating" @click="moveTrack(i, i - 1)"><ChevronUp /></UiButton>
              <UiButton
                variant="icon"
                title="Move down"
                aria-label="Move down"
                :disabled="i === playlists.detail.tracks.length - 1 || mutating"
                @click="moveTrack(i, i + 1)"
              >
                <ChevronDown />
              </UiButton>
              <UiButton variant="icon-danger" title="Remove from playlist" aria-label="Remove from playlist" :disabled="mutating" @click="removeTrack(t.id)">
                <X />
              </UiButton>
            </TrackRow>
          </div>
        </template>
        <template #overlay>
          <RowDragOverlay :drag-from="dragFrom" :drop-slot="slot" :row-height="ROW_HEIGHT" />
        </template>
      </TrackCollection>
    </template>
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
