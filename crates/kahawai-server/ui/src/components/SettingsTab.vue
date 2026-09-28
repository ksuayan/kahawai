<script setup lang="ts">
import { X } from "lucide-vue-next";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";
import { dirChipClass, dirChipText } from "../types";

const setup = useSetupStore();
</script>

<template>
  <div class="flex h-full flex-col px-8 py-8">
    <h2 class="heading-1 mb-2">Settings</h2>

    <h3 class="heading-3 mb-2 mt-2">Music folders</h3>
    <ul class="mb-3 flex flex-col gap-2">
      <li
        v-for="d in setup.runningDirs"
        :key="d.path"
        class="flex items-center justify-between gap-3 rounded-md border border-line bg-raised px-3 py-2"
      >
        <div class="min-w-0">
          <div class="flex items-center gap-2 truncate text-[13px]">
            {{ d.path }}
            <span
              v-if="setup.pendingAdds.includes(d.path)"
              class="micro-label rounded-sm bg-accent/15 px-1 py-0.5 text-accent"
            >
              new
            </span>
          </div>
          <div
            class="text-xs"
            :class="d.validation ? dirChipClass(d.validation) : 'text-faint'"
            :title="d.validation?.truncated ? 'This is a lower bound — the folder has more files than this quick check counts. The actual scan is never capped.' : undefined"
          >
            {{ d.validating ? "checking…" : d.validation ? dirChipText(d.validation) : "" }}
          </div>
        </div>
        <UiButton
          variant="icon-danger"
          aria-label="Remove folder"
          @click="setup.removeRunningDir(d.path)"
        >
          <X class="size-4" />
        </UiButton>
      </li>
    </ul>
    <div class="mb-2 flex flex-wrap items-center gap-2">
      <UiButton @click="setup.addRunningDirFromPicker()">Add folder…</UiButton>
      <UiButton
        variant="primary"
        :disabled="!setup.canApply || setup.applying"
        @click="setup.applyAndRescan()"
      >
        {{ setup.applying || setup.isScanning ? "Scanning…" : "Apply" }}
      </UiButton>
    </div>
    <UiHint v-if="setup.canApply" tone="faint">
      {{ setup.pendingAdds.length }} to add, {{ setup.pendingRemoves.length }} to remove — not
      applied until you click Apply. A folder is only ever dropped if you explicitly remove it.
    </UiHint>
    <UiHint v-if="setup.applyError" tone="warn">{{ setup.applyError }}</UiHint>

    <h3 class="heading-3 mb-2 mt-4">Advanced</h3>
    <UiHint tone="faint">
      Database: {{ setup.runningDbPath }} · Bind address: {{ setup.runningBind }}
    </UiHint>
    <div class="mt-2 flex flex-wrap gap-2">
      <UiButton @click="setup.editConfiguration()">Change bind/database (restart required)</UiButton>
      <UiButton @click="setup.revealConfig()">Reveal Config in Finder</UiButton>
    </div>
  </div>
</template>
