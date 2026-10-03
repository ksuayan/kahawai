<script setup lang="ts">
import { Trash2, UserPlus } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useAudiobooksStore } from "../stores/audiobooks";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";

/**
 * Who is listening on this Player: a voluntary name, kept on this device,
 * that the server keeps each person's place, bookmarks and speed under.
 */
const books = useAudiobooksStore();
const newListener = ref("");
const listenerOptions = (): UiSelectOption[] => books.listeners.map((l) => ({ value: l.name, label: l.name }));
const error = ref<string | null>(null);

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

async function choose(name: string | null): Promise<void> {
  error.value = null;
  try {
    await books.switchListener(name ?? "");
  } catch (e) {
    error.value = message(e);
  }
}

async function add(): Promise<void> {
  if (!newListener.value.trim()) return;
  error.value = null;
  try {
    await books.addListener(newListener.value);
    newListener.value = "";
  } catch (e) {
    error.value = message(e);
  }
}

async function removeCurrent(): Promise<void> {
  const cur = books.listeners.find((l) => l.name === (books.listener || "Default"));
  if (!cur || cur.id === 0) return;
  error.value = null;
  try {
    await books.removeListener(cur.id);
  } catch (e) {
    error.value = message(e);
  }
}

onMounted(async () => {
  try {
    await books.loadListeners();
  } catch (e) {
    error.value = message(e);
  }
});
</script>

<template>
  <div data-testid="listener-section">
    <UiHint>
      Who is listening on this Player. Everyone using the server shares the audiobooks, but each listener keeps their
      own place, bookmarks, history, speed and finished books. There are no passwords: pick who you are. Removing a
      listener forgets their progress.
    </UiHint>
    <div class="flex gap-2">
      <UiSelect
        aria-label="Listener"
        trigger-class="flex-1"
        :model-value="books.listener || 'Default'"
        :options="listenerOptions()"
        data-testid="listener-select"
        @update:model-value="choose"
      />
      <UiButton
        variant="icon-danger"
        title="Remove this listener and their progress"
        aria-label="Remove listener"
        :disabled="!books.listener"
        data-testid="remove-listener"
        @click="removeCurrent"
      ><Trash2 /></UiButton>
    </div>
    <div class="mt-2 flex gap-2">
      <UiInput v-model="newListener" class="flex-1" type="text" maxlength="40" placeholder="New listener's name" aria-label="New listener name" data-testid="new-listener" @keydown.enter="add" />
      <UiButton variant="primary" :disabled="!newListener.trim()" data-testid="add-listener" @click="add"><UserPlus class="mr-1" />Add</UiButton>
    </div>
    <StateMessage v-if="error" kind="error" class="mt-3">{{ error }}</StateMessage>
  </div>
</template>
