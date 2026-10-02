<script setup lang="ts">
import { RefreshCw, Trash2 } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useAudiobooksStore } from "../stores/audiobooks";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiInput from "../ui/UiInput.vue";

/**
 * Where the server finds audiobooks. The path is on the server's machine
 * (the Player may be on another one), so it is typed, not browsed.
 */
const books = useAudiobooksStore();
const path = ref("");
const name = ref("");
const error = ref<string | null>(null);
const busy = ref(false);

onMounted(async () => {
  try {
    await books.loadRoots();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
});

async function add(): Promise<void> {
  if (!path.value.trim()) return;
  busy.value = true;
  error.value = null;
  try {
    await books.addRoot(path.value.trim(), name.value.trim() || undefined);
    path.value = "";
    name.value = "";
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}

async function remove(id: number): Promise<void> {
  error.value = null;
  try {
    await books.removeRoot(id);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}

async function rescan(): Promise<void> {
  error.value = null;
  try {
    await books.rescan();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <div data-testid="audiobook-section">
    <UiHint>
      Folders of audiobooks on the server, kept apart from the music. Each folder of audio files is a book
      (<code>Author/Series/Vol 1 - 1999 - Title {Narrator}</code>); <code>Disc 1</code> and <code>CD 2</code> folders
      join their book. Removing a folder forgets its books and your progress; the files stay.
    </UiHint>
    <ul v-if="books.roots.length" class="m-0 mb-2 list-none p-0" data-testid="audiobook-roots">
      <li v-for="r in books.roots" :key="r.id" class="flex items-center gap-2 rounded-md px-2 py-1 hover:bg-hover">
        <div class="min-w-0 flex-1">
          <div class="truncate text-[13px] font-semibold">{{ r.name }}</div>
          <div class="truncate text-xs text-dim" :title="r.path">{{ r.path }}</div>
        </div>
        <UiButton variant="icon-danger" title="Remove this folder" aria-label="Remove folder" data-testid="remove-root" @click="remove(r.id)"><Trash2 /></UiButton>
      </li>
    </ul>
    <p v-else class="m-0 mb-2 text-xs text-faint">No audiobook folders yet.</p>
    <div class="flex gap-2">
      <UiInput v-model="path" class="flex-1" type="text" spellcheck="false" placeholder="/path/on/the/server/Audiobooks" aria-label="Audiobook folder path" data-testid="root-path" @keydown.enter="add" />
      <UiInput v-model="name" class="w-[140px]" type="text" placeholder="Name (optional)" aria-label="Folder name" />
      <UiButton variant="primary" :disabled="busy || !path.trim()" data-testid="add-root" @click="add">Add</UiButton>
      <UiButton variant="icon" title="Scan the audiobook folders again" aria-label="Rescan audiobooks" data-testid="rescan-books" @click="rescan"><RefreshCw /></UiButton>
    </div>
    <StateMessage v-if="error" kind="error" class="mt-3">{{ error }}</StateMessage>
  </div>
</template>
