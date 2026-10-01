<script setup lang="ts">
import { computed, onUnmounted, ref } from "vue";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";
import { describeServer, formatElapsed, formatWhen, scanFiles, scanTiming, type ScanJob } from "../types";

const setup = useSetupStore();

/** Ticks every second, so a running scan's elapsed time counts up. */
const now = ref(Date.now());
const clock = window.setInterval(() => (now.value = Date.now()), 1000);
onUnmounted(() => window.clearInterval(clock));

/** The other Kahawai Server's panel: stop asks for a confirmation first. */
const confirmStop = ref(false);
async function stopOther(): Promise<void> {
  confirmStop.value = false;
  await setup.stopOtherServer();
}

/** The running scan's live file counts, once the server has reported any. */
const runningFiles = computed(() => {
  const f = setup.recentScans.find((j) => j.status === "running")?.files;
  return f ? scanFiles(f, now.value) : null;
});

/** The content-hashing job's counts: the scan queues it when it finishes. */
const hashing = computed(() => {
  const j = setup.hashJob;
  if (!j) return null;
  return { running: j.status === "running", files: j.files ? scanFiles(j.files, now.value) : null };
});

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
    <p v-if="setup.serverStatus?.running" class="prose-text mb-1">
      Listening on {{ setup.serverStatus.bind }}.
    </p>
    <p
      v-if="setup.serverStatus?.running && setup.identity"
      class="mb-4 text-xs text-dim"
      :title="`Built ${setup.identity.build.built_at} · library ${setup.identity.catalog_id}`"
      data-testid="server-identity"
    >
      {{ describeServer(setup.identity) }}
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
    <section
      v-if="!setup.serverStatus?.running && setup.serverStatus?.occupant"
      class="mb-3 rounded-md border border-line bg-raised p-4"
      aria-labelledby="other-server-title"
      data-testid="other-server"
    >
      <h4 id="other-server-title" class="m-0 mb-1 text-[13px] font-semibold">
        Another Kahawai Server is running on {{ setup.serverStatus.bind }}
      </h4>
      <p class="m-0 mb-3 text-xs text-dim">
        This one can't start while it holds the port. Players connected to it (on this address) are
        using it now.
      </p>
      <dl class="m-0 mb-3 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-[13px]">
        <dt class="text-dim">Server</dt>
        <dd class="m-0">{{ describeServer(setup.serverStatus.occupant) }}</dd>
        <dt class="text-dim">Running since</dt>
        <dd class="m-0 tabular-nums">
          {{ formatWhen(setup.serverStatus.occupant.started_at) }} ({{
            formatElapsed(now - setup.serverStatus.occupant.started_at)
          }})
        </dd>
        <dt class="text-dim">Built</dt>
        <dd class="m-0 tabular-nums">{{ formatWhen(Date.parse(setup.serverStatus.occupant.build.built_at)) }}</dd>
        <dt class="text-dim">Library</dt>
        <dd class="m-0 break-all font-mono text-xs">{{ setup.serverStatus.occupant.catalog_id }}</dd>
      </dl>
      <div class="flex flex-wrap items-center gap-2">
        <template v-if="!confirmStop">
          <UiButton variant="primary" :disabled="setup.stoppingOther" @click="confirmStop = true">
            {{ setup.stoppingOther ? "Stopping it…" : "Stop it and start this server" }}
          </UiButton>
        </template>
        <template v-else>
          <span class="text-xs text-dim">Stop the other server? Players using it will lose it until they reconnect here.</span>
          <UiButton variant="danger" @click="stopOther">Stop it</UiButton>
          <UiButton @click="confirmStop = false">Cancel</UiButton>
        </template>
      </div>
      <p v-if="setup.startError" class="m-0 mt-2 text-xs text-danger-fg" role="alert">{{ setup.startError }}</p>
    </section>
    <!-- The panel above explains a port held by another Kahawai Server. -->
    <UiHint v-else-if="setup.startError" tone="warn">{{ setup.startError }}</UiHint>
    <UiHint
      v-else-if="!setup.serverStatus?.running && setup.serverStatus?.error"
      tone="warn"
      data-testid="server-error"
    >
      {{ setup.serverStatus.error }}
    </UiHint>

    <div v-if="setup.isScanning" class="mb-4 mt-2 rounded-md border border-line bg-raised p-3">
      <h3 class="heading-3 mb-2">Scanning…</h3>
      <div class="flex gap-6 text-[13px]">
        <div><span class="text-dim">Albums</span> <span class="font-semibold">{{ setup.liveScanStats?.albums ?? 0 }}</span></div>
        <div><span class="text-dim">Artists</span> <span class="font-semibold">{{ setup.liveScanStats?.artists ?? 0 }}</span></div>
        <div><span class="text-dim">Tracks</span> <span class="font-semibold">{{ setup.liveScanStats?.tracks ?? 0 }}</span></div>
      </div>
      <dl v-if="runningFiles" class="m-0 mt-2 flex flex-wrap gap-x-6 gap-y-1 text-[13px]" data-testid="scan-files">
        <div>
          <dt class="inline text-dim">Files processed</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="files-processed">{{ runningFiles.processed }}</dd>
        </div>
        <div v-if="runningFiles.remaining !== null">
          <dt class="inline text-dim">Files remaining to process</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="files-remaining">{{ runningFiles.remaining }}</dd>
        </div>
        <div v-if="runningFiles.rate">
          <dt class="inline text-dim">Rate</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="files-rate">{{ runningFiles.rate }}</dd>
        </div>
        <div v-if="runningFiles.eta">
          <dt class="inline text-dim">ETA</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="files-eta">{{ runningFiles.eta }}</dd>
        </div>
      </dl>
      <p v-if="setup.liveScanStats?.last_album" class="mt-2 truncate text-xs text-faint">
        Last added: {{ setup.liveScanStats.last_album
        }}<span v-if="setup.liveScanStats.last_album_artist"> — {{ setup.liveScanStats.last_album_artist }}</span>
      </p>
    </div>

    <div v-if="hashing" class="mb-4 mt-2 rounded-md border border-line bg-raised p-3" data-testid="hashing">
      <h3 class="heading-3 mb-2">{{ hashing.running ? "Hashing files…" : "Hashing queued" }}</h3>
      <dl v-if="hashing.files" class="m-0 flex flex-wrap gap-x-6 gap-y-1 text-[13px]">
        <div>
          <dt class="inline text-dim">Files processed</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="hash-processed">{{ hashing.files.processed }}</dd>
        </div>
        <div v-if="hashing.files.remaining !== null">
          <dt class="inline text-dim">Files remaining to process</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="hash-remaining">{{ hashing.files.remaining }}</dd>
        </div>
        <div v-if="hashing.files.mbps">
          <dt class="inline text-dim">Speed</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="hash-mbps">{{ hashing.files.mbps }}</dd>
        </div>
        <div v-if="hashing.files.eta">
          <dt class="inline text-dim">ETA</dt>
          <dd class="m-0 ml-1 inline font-semibold tabular-nums" data-testid="hash-eta">{{ hashing.files.eta }}</dd>
        </div>
      </dl>
      <p class="m-0 mt-2 text-xs text-faint">Checksums for finding duplicate copies. Playback and browsing work meanwhile.</p>
    </div>

    <h3 class="heading-3 mb-2 mt-2">Recent scans</h3>
    <ul v-if="setup.recentScans.length" class="mb-2 flex flex-col gap-1 text-[13px]">
      <li v-for="j in setup.recentScans" :key="j.id" class="flex flex-col" data-testid="recent-scan">
        <div class="flex items-baseline gap-2">
          <span class="w-20 shrink-0" :class="scanClass(j)">{{ scanLabel(j) }}</span>
          <span class="tabular-nums text-dim" data-testid="scan-timing">{{ scanTiming(j, now) }}</span>
        </div>
        <div class="min-h-[1.25rem] truncate pl-[5.5rem] text-xs text-faint" :title="j.message ?? undefined">
          {{ j.message ?? "" }}
        </div>
      </li>
    </ul>
    <UiHint v-else tone="faint">No scans yet.</UiHint>
  </div>
</template>
