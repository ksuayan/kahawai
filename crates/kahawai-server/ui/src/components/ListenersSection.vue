<script setup lang="ts">
import { X } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { setupAddAudiobookListener, setupAudiobookListeners, setupRemoveAudiobookListener } from "../tauri";
import type { AudiobookListener } from "../types";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiInput from "../ui/UiInput.vue";

/**
 * The people sharing the audiobooks. Each keeps their own place, bookmarks,
 * history, speed and finished books; each Player picks who is listening
 * (in its Audiobooks view). Making and removing them is a server matter, here.
 */
const listeners = ref<AudiobookListener[]>([]);
const name = ref("");
const error = ref<string | null>(null);

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

async function load(): Promise<void> {
  listeners.value = await setupAudiobookListeners();
}

async function add(): Promise<void> {
  if (!name.value.trim()) return;
  error.value = null;
  try {
    await setupAddAudiobookListener(name.value.trim());
    name.value = "";
    await load();
  } catch (e) {
    error.value = message(e);
  }
}

async function remove(id: number): Promise<void> {
  error.value = null;
  try {
    await setupRemoveAudiobookListener(id);
    await load();
  } catch (e) {
    error.value = message(e);
  }
}

onMounted(load);
</script>

<template>
  <div data-testid="listeners">
    <h3 class="heading-3 mb-2 mt-4">Audiobook listeners</h3>
    <UiHint tone="faint">
      Everyone shares the books, but each listener keeps their own place, bookmarks, history, speed and finished
      books. There are no passwords: each Player picks who is listening. Removing a listener forgets their
      progress; the books stay.
    </UiHint>
    <ul class="mb-3 flex flex-col gap-2">
      <li
        v-for="l in listeners"
        :key="l.id"
        class="flex items-center justify-between gap-3 rounded-md border border-line bg-raised px-3 py-2"
        data-testid="listener-row"
      >
        <div class="min-w-0">
          <div class="truncate text-[13px]">{{ l.name }}</div>
          <div class="text-xs text-faint">
            {{ l.books_started === 1 ? "1 book started" : `${l.books_started} books started` }}
          </div>
        </div>
        <UiButton
          v-if="l.id !== 0"
          variant="icon-danger"
          :aria-label="`Remove ${l.name}`"
          data-testid="remove-listener"
          @click="remove(l.id)"
        >
          <X class="size-4" />
        </UiButton>
      </li>
    </ul>
    <div class="mb-2 flex flex-wrap items-center gap-2">
      <UiInput
        v-model="name"
        class="w-[220px]"
        maxlength="40"
        placeholder="New listener's name"
        aria-label="New listener's name"
        data-testid="new-listener"
        @keydown.enter="add"
      />
      <UiButton :disabled="!name.trim()" data-testid="add-listener" @click="add">Add listener</UiButton>
    </div>
    <UiHint v-if="error" tone="warn">{{ error }}</UiHint>
  </div>
</template>
