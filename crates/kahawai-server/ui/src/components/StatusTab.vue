<script setup lang="ts">
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";
import type { ScanJob } from "../types";

const setup = useSetupStore();

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
  <div class="flex h-full flex-col px-8 py-8">
    <h2 class="heading-1 mb-2">
      {{ setup.serverStatus?.running ? "Server running" : "Server not running" }}
    </h2>
    <p v-if="setup.serverStatus?.running" class="prose-text mb-4">
      Listening on {{ setup.serverStatus.bind }}.
    </p>

    <div class="mb-2 flex flex-wrap gap-2">
      <UiButton
        v-if="setup.serverStatus?.running"
        variant="danger"
        @click="setup.stopServer()"
      >
        Stop Server
      </UiButton>
      <UiButton v-else variant="primary" @click="setup.startServerAndContinue()">
        Start Server
      </UiButton>
      <UiButton @click="setup.restartServer()">Restart Server</UiButton>
      <UiButton variant="danger" @click="setup.quit()">Quit App</UiButton>
    </div>
    <UiHint v-if="setup.startError" tone="warn">{{ setup.startError }}</UiHint>

    <div v-if="setup.isScanning" class="mb-4 mt-2 rounded-md border border-line bg-raised p-3">
      <h3 class="heading-3 mb-2">Scanning…</h3>
      <div class="flex gap-6 text-[13px]">
        <div><span class="text-dim">Albums</span> <span class="font-semibold">{{ setup.liveScanStats?.albums ?? 0 }}</span></div>
        <div><span class="text-dim">Artists</span> <span class="font-semibold">{{ setup.liveScanStats?.artists ?? 0 }}</span></div>
        <div><span class="text-dim">Tracks</span> <span class="font-semibold">{{ setup.liveScanStats?.tracks ?? 0 }}</span></div>
      </div>
      <p v-if="setup.liveScanStats?.last_album" class="mt-2 truncate text-xs text-faint">
        Last added: {{ setup.liveScanStats.last_album
        }}<span v-if="setup.liveScanStats.last_album_artist"> — {{ setup.liveScanStats.last_album_artist }}</span>
      </p>
    </div>

    <h3 class="heading-3 mb-2 mt-2">Recent scans</h3>
    <ul v-if="setup.recentScans.length" class="mb-2 flex flex-col gap-1 text-[13px]">
      <li v-for="j in setup.recentScans" :key="j.id" class="flex items-start justify-between gap-3">
        <span :class="scanClass(j)">{{ scanLabel(j) }}</span>
        <span class="min-w-0 flex-1 truncate text-right text-dim" :title="j.message ?? undefined">
          {{ j.message ?? "" }}
        </span>
      </li>
    </ul>
    <UiHint v-else tone="faint">No scans yet.</UiHint>
  </div>
</template>
