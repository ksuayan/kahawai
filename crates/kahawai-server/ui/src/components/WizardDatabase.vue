<script setup lang="ts">
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";

const setup = useSetupStore();
</script>

<template>
  <div>
    <h2 class="heading-1 mb-2">Database</h2>
    <p class="prose-text mb-4">Pick a folder to hold the music catalog database.</p>

    <div class="mb-3 flex items-center gap-3">
      <UiButton @click="setup.pickDbDir()">Choose folder…</UiButton>
      <span v-if="setup.dbDir" class="truncate text-[13px] text-dim">{{ setup.dbDir }}</span>
    </div>

    <UiHint v-if="setup.dbDir" tone="faint">
      Catalog will be stored at {{ setup.dbDir }}/music.db
    </UiHint>
    <UiHint v-if="setup.dbDir && setup.dbDirValidation && !setup.dbDirValidation.writable" tone="warn">
      This folder is not writable — choose another.
    </UiHint>
  </div>
</template>
