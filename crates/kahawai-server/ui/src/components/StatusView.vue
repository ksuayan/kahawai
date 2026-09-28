<script setup lang="ts">
import { X } from "lucide-vue-next";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";
import { dirStatus } from "../types";
import type { DirValidation, ScanJob } from "../types";

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

function scanLabel(j: ScanJob): string {
  if (j.status === "failed") return "Failed";
  if (j.status === "done") return "Done";
  return j.status === "running" ? "Running…" : "Queued";
}

function scanClass(j: ScanJob): string {
  return j.status === "failed" ? "text-danger-fg" : "text-dim";
}
</script>

<template>
  <div class="flex h-full flex-col overflow-y-auto px-8 py-8">
    <h2 class="heading-1 mb-2">
      {{ setup.serverStatus?.running ? "Server running" : "Server not running" }}
    </h2>
    <p v-if="setup.serverStatus?.running" class="prose-text mb-4">
      Listening on {{ setup.serverStatus.bind }}.
    </p>

    <h3 class="heading-3 mb-2 mt-2">Music folders</h3>
    <ul class="mb-3 flex flex-col gap-2">
      <li
        v-for="d in setup.runningDirs"
        :key="d.path"
        class="flex items-center justify-between gap-3 rounded-md border border-line bg-raised px-3 py-2"
      >
        <div class="min-w-0">
          <div class="truncate text-[13px]">{{ d.path }}</div>
          <div class="text-xs" :class="d.validation ? chipClass(d.validation) : 'text-faint'">
            {{ d.validating ? "checking…" : d.validation ? chipText(d.validation) : "" }}
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
      <UiButton variant="primary" :disabled="!setup.canApply || setup.applying" @click="setup.applyAndRescan()">
        {{ setup.applying ? "Scanning…" : "Apply" }}
      </UiButton>
    </div>
    <UiHint v-if="setup.applyError" tone="warn">{{ setup.applyError }}</UiHint>
    <UiHint tone="faint" spaced>
      Database: {{ setup.runningDbPath }} · Bind address: {{ setup.runningBind }} (changing these
      needs the Advanced settings below and a restart).
    </UiHint>

    <h3 class="heading-3 mb-2 mt-4">Recent scans</h3>
    <ul v-if="setup.recentScans.length" class="mb-2 flex flex-col gap-1 text-[13px]">
      <li v-for="j in setup.recentScans" :key="j.id" class="flex items-start justify-between gap-3">
        <span :class="scanClass(j)">{{ scanLabel(j) }}</span>
        <span class="min-w-0 flex-1 truncate text-right text-dim" :title="j.message ?? undefined">
          {{ j.message ?? "" }}
        </span>
      </li>
    </ul>
    <UiHint v-else tone="faint">No scans yet.</UiHint>

    <div class="mt-4 flex flex-wrap gap-2">
      <UiButton @click="setup.editConfiguration()">Advanced settings (restart required)</UiButton>
      <UiButton @click="setup.revealConfig()">Reveal Config in Finder</UiButton>
      <UiButton variant="danger" @click="setup.quit()">Quit Server</UiButton>
    </div>
  </div>
</template>
