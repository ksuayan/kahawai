<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useToastsStore } from "../stores/toasts";

const nav = useNavStore();
const playlists = usePlaylistsStore();
const toasts = useToastsStore();

const creating = ref(false);
const newName = ref("");
const renamingId = ref<number | null>(null);
const renameText = ref("");
const actionError = ref<string | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);
const importing = ref(false);

onMounted(() => {
  if (!playlists.loaded) void playlists.load();
});

function fail(e: unknown): void {
  actionError.value = e instanceof Error ? e.message : String(e);
}

async function doCreate(): Promise<void> {
  const name = newName.value.trim();
  if (!name) return;
  actionError.value = null;
  try {
    const pl = await playlists.create(name);
    newName.value = "";
    creating.value = false;
    nav.go("playlist", pl.id);
  } catch (e) {
    fail(e);
  }
}

function startRename(p: { id: number; name: string }): void {
  renamingId.value = p.id;
  renameText.value = p.name;
  actionError.value = null;
}

async function commitRename(id: number): Promise<void> {
  const name = renameText.value.trim();
  renamingId.value = null;
  if (!name) return;
  try {
    await playlists.rename(id, name);
  } catch (e) {
    fail(e);
    toasts.push("error", "Rename failed", { detail: e instanceof Error ? e.message : String(e) });
  }
}

async function confirmDelete(id: number, name: string): Promise<void> {
  if (!window.confirm(`Delete playlist "${name}"?`)) return;
  try {
    await playlists.remove(id);
  } catch (e) {
    fail(e);
  }
}

function pickFile(): void {
  fileInput.value?.click();
}

async function onFilePicked(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  if (!/\.m3u8?$/i.test(file.name)) {
    toasts.push("error", "Not an M3U file", { detail: "Pick a .m3u or .m3u8 playlist file." });
    return;
  }
  importing.value = true;
  try {
    await playlists.importFile(file);
  } catch (e) {
    toasts.push("error", "Playlist import failed", {
      detail: e instanceof Error ? e.message : String(e),
    });
  } finally {
    importing.value = false;
  }
}

function closeImportDialog(): void {
  playlists.clearImport();
}
</script>

<template>
  <div class="view">
    <div class="header">
      <div>
        <h2>Playlists</h2>
        <p class="sub">{{ playlists.items.length }} playlists</p>
      </div>
      <div class="actions">
        <button v-if="!creating" class="primary" @click="creating = true">＋ New playlist</button>
        <button :disabled="importing" @click="pickFile">
          {{ importing ? "Importing…" : "Import M3U…" }}
        </button>
        <input
          ref="fileInput"
          type="file"
          accept=".m3u,.m3u8"
          class="hidden"
          @change="onFilePicked"
        />
      </div>
    </div>

    <div v-if="creating" class="inline-form">
      <input
        v-model="newName"
        type="text"
        placeholder="Playlist name"
        maxlength="120"
        @keydown.enter="doCreate"
        @keydown.escape="creating = false"
      />
      <button class="primary" :disabled="!newName.trim()" @click="doCreate">Create</button>
      <button @click="creating = false">Cancel</button>
    </div>

    <div v-if="actionError" class="error-banner">{{ actionError }}</div>
    <div v-if="playlists.loading" class="spinner">Loading playlists…</div>
    <div v-else-if="playlists.error" class="error-banner">{{ playlists.error }}</div>
    <div v-else-if="playlists.items.length === 0 && !creating" class="empty">
      No playlists yet. Create one, import an M3U file, or save the queue as a playlist.
    </div>
    <ul v-else class="list">
      <li v-for="p in playlists.items" :key="p.id" class="row">
        <template v-if="renamingId === p.id">
          <input
            v-model="renameText"
            type="text"
            class="rename-input"
            maxlength="120"
            @keydown.enter="commitRename(p.id)"
            @keydown.escape="renamingId = null"
          />
          <button class="primary" @click="commitRename(p.id)">Save</button>
          <button @click="renamingId = null">Cancel</button>
        </template>
        <template v-else>
          <button class="name" @click="nav.go('playlist', p.id)">{{ p.name }}</button>
          <span class="meta">{{ p.track_ids.length }} tracks</span>
          <button class="icon-btn" title="Rename playlist" @click="startRename(p)">✎</button>
          <button class="icon-btn danger" title="Delete playlist" @click="confirmDelete(p.id, p.name)">
            ✕
          </button>
        </template>
      </li>
    </ul>

    <!-- M3U import result dialog -->
    <div v-if="playlists.lastImport" class="dialog-backdrop" @click.self="closeImportDialog">
      <div class="dialog" role="dialog" aria-label="Playlist import result">
        <h3>Imported “{{ playlists.lastImport.name }}”</h3>
        <p>
          Matched <strong>{{ playlists.lastImport.result.matched }}</strong>
          track{{ playlists.lastImport.result.matched === 1 ? "" : "s" }}.
        </p>
        <template v-if="playlists.lastImport.result.unmatched.length > 0">
          <p class="sub">
            {{ playlists.lastImport.result.unmatched.length }} entr{{
              playlists.lastImport.result.unmatched.length === 1 ? "y" : "ies"
            }}
            did not match the library:
          </p>
          <ul class="unmatched">
            <li v-for="(u, i) in playlists.lastImport.result.unmatched" :key="i">{{ u }}</li>
          </ul>
        </template>
        <p v-else class="sub">Every entry matched the library.</p>
        <div class="dialog-actions">
          <button
            class="primary"
            @click="nav.go('playlist', playlists.lastImport!.result.playlist_id); closeImportDialog()"
          >
            Open playlist
          </button>
          <button @click="closeImportDialog">Close</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  margin-bottom: 12px;
}

.header h2 {
  margin: 0 0 4px;
}

.actions {
  display: flex;
  gap: 8px;
}

.hidden {
  display: none;
}

.inline-form {
  display: flex;
  gap: 8px;
  margin-bottom: 12px;
  max-width: 640px;
}

.inline-form input {
  flex: 1;
}

.list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-width: 640px;
}

.row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-radius: 6px;
}

.row:hover {
  background: var(--bg-hover);
}

.name {
  flex: 1;
  background: transparent;
  border: none;
  text-align: left;
  font-size: 14px;
  padding: 0;
  cursor: pointer;
  color: var(--text);
}

.meta {
  color: var(--text-dim);
  font-size: 12px;
}

.rename-input {
  flex: 1;
}

.dialog-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 80;
}

.dialog {
  background: var(--bg-raised);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 20px;
  max-width: 480px;
  width: calc(100% - 64px);
  max-height: 70vh;
  overflow-y: auto;
}

.dialog h3 {
  margin: 0 0 8px;
}

.unmatched {
  margin: 8px 0;
  padding-left: 20px;
  font-size: 12px;
  color: var(--text-dim);
  max-height: 180px;
  overflow-y: auto;
  word-break: break-all;
}

.dialog-actions {
  display: flex;
  gap: 8px;
  margin-top: 16px;
  justify-content: flex-end;
}
</style>
