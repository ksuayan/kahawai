<script setup lang="ts">
import { Pencil, Plus, X } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useMainScrollMemory } from "../lib/mainScroll";
import { useNavStore } from "../stores/nav";
import { usePlaylistsStore } from "../stores/playlists";
import { useToastsStore } from "../stores/toasts";
import ConfirmDialog from "../ui/ConfirmDialog.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import UiInput from "../ui/UiInput.vue";
import ViewShell from "../ui/ViewShell.vue";

const nav = useNavStore();
const playlists = usePlaylistsStore();
const toasts = useToastsStore();

useMainScrollMemory("playlists", () => playlists.loaded && !playlists.loading);

const creating = ref(false);
const newName = ref("");
const renamingId = ref<number | null>(null);
const renameText = ref("");
const actionError = ref<string | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);
const importing = ref(false);
const pendingDelete = ref<{ id: number; name: string } | null>(null);

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

async function confirmDelete(): Promise<void> {
  const target = pendingDelete.value;
  pendingDelete.value = null;
  if (!target) return;
  try {
    await playlists.remove(target.id);
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
  <ViewShell title="Playlists" :subtitle="`${playlists.items.length} playlists`">
    <template #actions>
      <UiButton v-if="!creating" variant="primary" @click="creating = true"><Plus /> New playlist</UiButton>
      <UiButton :disabled="importing" @click="pickFile">
        {{ importing ? "Importing…" : "Import M3U…" }}
      </UiButton>
      <input ref="fileInput" type="file" accept=".m3u,.m3u8" class="hidden" @change="onFilePicked" />
    </template>

    <div v-if="creating" class="mb-3 flex max-w-[640px] gap-2">
      <UiInput
        v-model="newName"
        class="flex-1"
        type="text"
        placeholder="Playlist name"
        aria-label="Playlist name"
        maxlength="120"
        @keydown.enter="doCreate"
        @keydown.escape="creating = false"
      />
      <UiButton variant="primary" :disabled="!newName.trim()" @click="doCreate">Create</UiButton>
      <UiButton @click="creating = false">Cancel</UiButton>
    </div>

    <StateMessage v-if="actionError" kind="error">{{ actionError }}</StateMessage>
    <StateMessage v-if="playlists.loading" kind="loading">Loading playlists…</StateMessage>
    <StateMessage v-else-if="playlists.error" kind="error">{{ playlists.error }}</StateMessage>
    <StateMessage v-else-if="playlists.items.length === 0 && !creating" kind="empty">
      No playlists yet. Create one, import an M3U file, or save the queue as a playlist.
    </StateMessage>
    <ul v-else class="m-0 flex max-w-[640px] list-none flex-col gap-0.5 p-0">
      <li
        v-for="p in playlists.items"
        :key="p.id"
        class="flex items-center gap-2 rounded-md px-2.5 py-2 hover:bg-hover"
        data-testid="playlist-row"
      >
        <template v-if="renamingId === p.id">
          <UiInput
            v-model="renameText"
            class="flex-1"
            type="text"
            aria-label="New playlist name"
            maxlength="120"
            @keydown.enter="commitRename(p.id)"
            @keydown.escape="renamingId = null"
          />
          <UiButton variant="primary" @click="commitRename(p.id)">Save</UiButton>
          <UiButton @click="renamingId = null">Cancel</UiButton>
        </template>
        <template v-else>
          <button
            type="button"
            class="flex-1 cursor-pointer border-0 bg-transparent p-0 text-left text-sm text-fg"
            @click="nav.go('playlist', p.id)"
          >
            {{ p.name }}
          </button>
          <span class="text-xs text-dim">{{ p.track_ids.length }} tracks</span>
          <UiButton variant="icon" title="Rename playlist" aria-label="Rename playlist" @click="startRename(p)"><Pencil /></UiButton>
          <UiButton variant="icon-danger"
            title="Delete playlist"
            aria-label="Delete playlist"
            @click="pendingDelete = { id: p.id, name: p.name }"
          >
            <X />
          </UiButton>
        </template>
      </li>
    </ul>

    <ConfirmDialog
      :open="pendingDelete !== null"
      title="Delete playlist?"
      :description="pendingDelete ? `“${pendingDelete.name}” will be permanently deleted.` : ''"
      confirm-label="Delete"
      danger
      @update:open="(v) => !v && (pendingDelete = null)"
      @confirm="confirmDelete"
    />

    <!-- M3U import result -->
    <UiDialog
      :open="playlists.lastImport !== null"
      :title="playlists.lastImport ? `Imported “${playlists.lastImport.name}”` : ''"
      @update:open="(v) => !v && closeImportDialog()"
    >
      <template v-if="playlists.lastImport">
        <p class="m-0">
          Matched <strong>{{ playlists.lastImport.result.matched }}</strong>
          track{{ playlists.lastImport.result.matched === 1 ? "" : "s" }}.
        </p>
        <template v-if="playlists.lastImport.result.unmatched.length > 0">
          <p class="mb-0 mt-2 text-dim">
            {{ playlists.lastImport.result.unmatched.length }} entr{{
              playlists.lastImport.result.unmatched.length === 1 ? "y" : "ies"
            }}
            did not match the library:
          </p>
          <ul class="my-2 max-h-44 list-disc overflow-y-auto break-all pl-5 text-xs text-dim" data-testid="unmatched">
            <li v-for="(u, i) in playlists.lastImport.result.unmatched" :key="i">{{ u }}</li>
          </ul>
        </template>
        <p v-else class="mb-0 mt-2 text-dim">Every entry matched the library.</p>
      </template>
      <template #footer>
        <UiButton
          variant="primary"
          @click="nav.go('playlist', playlists.lastImport!.result.playlist_id); closeImportDialog()"
        >
          Open playlist
        </UiButton>
        <UiButton @click="closeImportDialog">Close</UiButton>
      </template>
    </UiDialog>
  </ViewShell>
</template>
