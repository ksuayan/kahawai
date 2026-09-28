<script setup lang="ts">
import { X } from "lucide-vue-next";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";
import { dirStatus } from "../types";
import type { DirValidation } from "../types";

const setup = useSetupStore();

function chipText(v: DirValidation): string {
  const status = dirStatus(v);
  if (status === "err") return "not accessible";
  if (status === "warn") return "no audio files found";
  return `${v.audio_files} audio file${v.audio_files === 1 ? "" : "s"}`;
}

function chipClass(v: DirValidation): string {
  const status = dirStatus(v);
  if (status === "err") return "text-danger-fg";
  if (status === "warn") return "text-warn-fg";
  return "text-ok";
}
</script>

<template>
  <div>
    <h2 class="heading-1 mb-2">Music folders</h2>
    <p class="prose-text mb-4">
      Pick one or more folders to scan for music. At least one must be accessible with audio
      files in it.
    </p>

    <ul class="mb-3 flex flex-col gap-2">
      <li
        v-for="d in setup.dirs"
        :key="d.path"
        class="flex items-center justify-between gap-3 rounded-md border border-line bg-raised px-3 py-2"
      >
        <div class="min-w-0">
          <div class="truncate text-[13px]">{{ d.path }}</div>
          <div class="text-xs" :class="d.validation ? chipClass(d.validation) : 'text-faint'">
            {{ d.validating ? "checking…" : d.validation ? chipText(d.validation) : "" }}
          </div>
        </div>
        <UiButton variant="icon-danger" aria-label="Remove folder" @click="setup.removeDir(d.path)">
          <X class="size-4" />
        </UiButton>
      </li>
    </ul>

    <UiButton @click="setup.addDirFromPicker()">Add folder…</UiButton>

    <UiHint v-if="setup.okDirCount === 0" tone="warn" spaced>
      Add at least one accessible folder with audio files to continue.
    </UiHint>
  </div>
</template>
