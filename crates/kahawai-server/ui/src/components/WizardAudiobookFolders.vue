<script setup lang="ts">
import { X } from "lucide-vue-next";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";
import { bookDirChipClass, bookDirChipText } from "../types";

const setup = useSetupStore();
</script>

<template>
  <div data-testid="wizard-audiobooks">
    <h2 class="heading-1 mb-2">Audiobook folders</h2>
    <p class="prose-text mb-4">
      Optional. Audiobooks are kept apart from the music: they get their own library in the Player, with
      chapters, bookmarks and a remembered place in every book. Each folder of audio files is one book
      (<code>Author/Series/Vol 1 - Title {Narrator}</code>); <code>Disc 1</code> and <code>CD 2</code>
      folders join their book. Skip this step if you have none.
    </p>

    <ul class="mb-3 flex flex-col gap-2">
      <li
        v-for="d in setup.audiobookDirs"
        :key="d.path"
        class="flex items-center justify-between gap-3 rounded-md border border-line bg-raised px-3 py-2"
        data-testid="audiobook-dir"
      >
        <div class="min-w-0">
          <div class="truncate text-[13px]">{{ d.path }}</div>
          <div class="text-xs" :class="d.validation ? bookDirChipClass(d.validation) : 'text-faint'" data-testid="audiobook-chip">
            {{ d.validating ? "checking…" : d.validation ? bookDirChipText(d.validation) : "" }}
          </div>
        </div>
        <UiButton variant="icon-danger" aria-label="Remove folder" @click="setup.removeAudiobookDir(d.path)">
          <X class="size-4" />
        </UiButton>
      </li>
    </ul>

    <UiButton data-testid="add-audiobook-folder" @click="setup.addAudiobookDirFromPicker()">Add folder…</UiButton>

    <UiHint v-if="setup.audiobookDirs.length === 0" tone="faint" spaced>
      No audiobook folders. You can add some later in Settings.
    </UiHint>
  </div>
</template>
