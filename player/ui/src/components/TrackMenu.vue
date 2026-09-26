<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useJobsStore } from "../stores/jobs";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import { isPlayable, trackTitle, type Track } from "../types";

/**
 * Reusable "⋯" action menu for a track or a track list (album).
 * Actions: play next, add to queue, add to playlist, extract SACD ISO.
 */
const props = withDefaults(
  defineProps<{
    /** Single track context. */
    track?: Track | null;
    /** Multi-track context (album). `track` wins when both are given. */
    tracks?: Track[] | null;
    albumId?: number | null;
  }>(),
  { track: null, tracks: null, albumId: null },
);

const queue = useQueueStore();
const playlists = usePlaylistsStore();
const jobs = useJobsStore();
const toasts = useToastsStore();

const open = ref(false);
const playlistPicker = ref(false);
const busy = ref(false);

const list = computed<Track[]>(() => {
  if (props.track) return [props.track];
  return (props.tracks ?? []).filter(isPlayable);
});

const canExtractIso = computed(
  () => props.track?.format === "sacd_iso" && !props.track.missing,
);

onMounted(() => {
  // One shared fetch no matter how many menus mount; the store dedups.
  if (!playlists.loaded) void playlists.load();
});

function close(): void {
  open.value = false;
  playlistPicker.value = false;
}

function toggle(): void {
  open.value = !open.value;
  if (open.value) playlistPicker.value = false;
}

async function playNext(): Promise<void> {
  close();
  try {
    await queue.playNext(list.value);
    toasts.push("success", `Playing next: ${list.value.length} track${list.value.length === 1 ? "" : "s"}`);
  } catch (e) {
    toasts.push("error", "Play next failed", { detail: e instanceof Error ? e.message : String(e) });
  }
}

async function addToQueue(): Promise<void> {
  close();
  try {
    await queue.appendTracks(list.value);
    toasts.push("success", `Added ${list.value.length} track${list.value.length === 1 ? "" : "s"} to queue`);
  } catch (e) {
    toasts.push("error", "Add to queue failed", { detail: e instanceof Error ? e.message : String(e) });
  }
}

async function addToPlaylist(id: number): Promise<void> {
  busy.value = true;
  try {
    if (props.albumId != null && !props.track) {
      await playlists.addAlbum(id, props.albumId);
    } else {
      await playlists.addTracks(id, list.value.map((t) => t.id));
    }
  } catch (e) {
    toasts.push("error", "Add to playlist failed", { detail: e instanceof Error ? e.message : String(e) });
  } finally {
    busy.value = false;
    close();
  }
}

async function createAndAdd(): Promise<void> {
  const name = window.prompt("New playlist name:");
  if (!name || !name.trim()) return;
  busy.value = true;
  try {
    const pl = await playlists.create(name.trim());
    await addToPlaylist(pl.id);
  } catch (e) {
    toasts.push("error", "Could not create playlist", { detail: e instanceof Error ? e.message : String(e) });
    close();
  } finally {
    busy.value = false;
  }
}

function extractIso(): void {
  close();
  if (props.track) void jobs.extractIso(props.track);
}

const menuLabel = computed(() => {
  const t = props.track;
  return t ? `Actions for ${trackTitle(t)}` : "Actions";
});
</script>

<template>
  <div class="menu-wrap" @keydown.escape="close" @dblclick.stop>
    <button class="icon-btn" :title="menuLabel" @click.stop="toggle">⋯</button>
    <div v-if="open" class="menu" role="menu">
      <template v-if="!playlistPicker">
        <button role="menuitem" :disabled="list.length === 0 || busy" @click="playNext">
          ⏭ Play next
        </button>
        <button role="menuitem" :disabled="list.length === 0 || busy" @click="addToQueue">
          ☰ Add to queue
        </button>
        <button role="menuitem" :disabled="list.length === 0 || busy" @click="playlistPicker = true">
          ♫ Add to playlist…
        </button>
        <button v-if="canExtractIso" role="menuitem" :disabled="busy" @click="extractIso">
          💿 Extract to DSF
        </button>
      </template>
      <template v-else>
        <div class="menu-head">Add to playlist</div>
        <button
          v-for="p in playlists.items"
          :key="p.id"
          role="menuitem"
          :disabled="busy"
          @click="addToPlaylist(p.id)"
        >
          {{ p.name }} <span class="meta">({{ p.track_ids.length }})</span>
        </button>
        <button role="menuitem" :disabled="busy" class="new" @click="createAndAdd">
          ＋ New playlist…
        </button>
        <button role="menuitem" @click="playlistPicker = false">‹ Back</button>
      </template>
    </div>
  </div>
</template>

<style scoped>
.menu-wrap {
  position: relative;
  display: inline-block;
}

.menu {
  position: absolute;
  right: 0;
  top: 100%;
  margin-top: 4px;
  min-width: 200px;
  max-height: 320px;
  overflow-y: auto;
  background: var(--bg-raised);
  border: 1px solid var(--border);
  border-radius: 10px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  z-index: 60;
  padding: 4px;
  display: flex;
  flex-direction: column;
}

.menu button[role="menuitem"] {
  background: transparent;
  border: none;
  text-align: left;
  padding: 8px 10px;
  border-radius: 6px;
  font-size: 13px;
  cursor: pointer;
  color: var(--text);
  white-space: nowrap;
}

.menu button[role="menuitem"]:hover:not(:disabled) {
  background: var(--bg-hover);
}

.menu button[role="menuitem"]:disabled {
  opacity: 0.4;
  cursor: default;
}

.menu-head {
  font-size: 11px;
  color: var(--text-dim);
  padding: 6px 10px 4px;
  text-transform: uppercase;
  letter-spacing: 0.4px;
}

.menu .meta {
  color: var(--text-faint);
  font-size: 11px;
}

.menu .new {
  border-top: 1px solid var(--border);
  border-radius: 0;
  margin-top: 4px;
}
</style>
