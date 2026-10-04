<script setup lang="ts">
import { BookOpen, Disc3 } from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref } from "vue";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import { useSetupStore } from "../stores/setup";
import { describeServer, formatElapsed, formatWhen, isJobActive, scanFiles, scanTiming, type ScanJob } from "../types";

const setup = useSetupStore();

/** Ticks every second, so a running scan's elapsed time counts up. The
 *  library counts and connected Players are read every few seconds (a scan
 *  polls faster on its own). */
const now = ref(Date.now());
let ticks = 0;
const clock = window.setInterval(() => {
  now.value = Date.now();
  if (++ticks % 5 === 0 && setup.serverStatus?.running) void setup.refreshLiveScanStats();
}, 1000);
onMounted(() => {
  if (setup.serverStatus?.running && !setup.liveScanStats) void setup.refreshLiveScanStats();
});
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

/** The scan that is running: an audiobook scan says so, and counts books, not albums. */
const runningScan = computed(() => setup.recentScans.find(isJobActive));
const scanningBooks = computed(() => runningScan.value?.label.toLowerCase().includes("audiobook") ?? false);
const lookup = computed(() => setup.bookLookupJob);

/** How far the running scan is, 0–100: from the file counts when the previous
 *  scan's total is known, else the job's own progress; null before either. */
const scanPercent = computed(() => {
  const j = runningScan.value;
  if (!j) return null;
  const f = j.files;
  if (f?.total) return Math.min(100, Math.round((f.done / f.total) * 100));
  return j.progress > 0 ? Math.round(j.progress * 100) : null;
});

const stats = computed(() => setup.liveScanStats);
const players = computed(() => stats.value?.players ?? 0);

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
      {{ setup.serverStatus?.running ? "Server running" : setup.serverStatus?.starting ? "Server starting…" : "Server not running" }}
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

    <div v-if="setup.serverStatus?.running" class="mb-4 mt-2 rounded-md border border-line bg-raised p-3" data-testid="library-panel">
      <h3 class="heading-3 mb-2" data-testid="scan-heading">
        {{ setup.isScanning ? (scanningBooks ? "Scanning audiobooks…" : "Scanning…") : "Library" }}
      </h3>
      <div class="flex flex-wrap gap-x-6 gap-y-1 text-[13px]">
        <div><span class="text-dim">Albums</span> <span class="font-semibold tabular-nums">{{ stats?.albums ?? 0 }}</span></div>
        <div><span class="text-dim">Artists</span> <span class="font-semibold tabular-nums">{{ stats?.artists ?? 0 }}</span></div>
        <div><span class="text-dim">Tracks</span> <span class="font-semibold tabular-nums">{{ stats?.tracks ?? 0 }}</span></div>
        <div data-testid="books-count">
          <span class="text-dim">Audiobooks</span> <span class="font-semibold tabular-nums">{{ stats?.audiobooks ?? 0 }}</span>
        </div>
        <div data-testid="players-count">
          <span class="text-dim">{{ players === 1 ? "Player connected" : "Players connected" }}</span> <span class="font-semibold tabular-nums">{{ players }}</span>
        </div>
      </div>

      <template v-if="setup.isScanning">
        <div v-if="scanPercent !== null" class="mt-3 flex items-center gap-3" data-testid="scan-progress">
          <div
            class="h-1.5 flex-1 overflow-hidden rounded-full bg-active"
            role="progressbar"
            :aria-valuenow="scanPercent"
            aria-valuemin="0"
            aria-valuemax="100"
            aria-label="Scan progress"
          >
            <div class="h-full rounded-full bg-accent transition-[width] duration-500" :style="{ width: `${scanPercent}%` }" />
          </div>
          <span class="w-10 text-right text-[13px] font-semibold tabular-nums" data-testid="scan-percent">{{ scanPercent }}%</span>
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
        <p v-if="!scanningBooks && stats?.last_album" class="mt-2 truncate text-xs text-faint">
          Last added: {{ stats.last_album }}<span v-if="stats.last_album_artist"> — {{ stats.last_album_artist }}</span>
        </p>
      </template>

      <div class="mt-3 flex flex-wrap items-center gap-2 border-t border-line pt-3">
        <span class="text-[13px] text-dim">Rescan</span>
        <UiButton :disabled="setup.isScanning" title="Scan the music folders again (audiobook folders too)" data-testid="rescan-library" @click="setup.rescan('library')">
          <Disc3 class="size-4" /> Library
        </UiButton>
        <UiButton :disabled="setup.isScanning" title="Scan the audiobook folders again" data-testid="rescan-audiobooks" @click="setup.rescan('audiobooks')">
          <BookOpen class="size-4" /> Audiobooks
        </UiButton>
      </div>
      <p v-if="setup.rescanError" class="m-0 mt-2 text-xs text-danger-fg" role="alert">{{ setup.rescanError }}</p>
    </div>

    <div v-if="lookup" class="mb-4 mt-2 rounded-md border border-line bg-raised p-3" data-testid="book-lookup">
      <h3 class="heading-3 mb-2">Looking up audiobook details…</h3>
      <p class="m-0 text-[13px] tabular-nums">
        <span class="text-dim">Progress</span>
        <span class="ml-1 font-semibold" data-testid="lookup-percent">{{ Math.round(lookup.progress * 100) }}%</span>
      </p>
      <p class="m-0 mt-2 text-xs text-faint">Fills a missing author, year or cover from Open Library and Google Books. Your tags are never changed.</p>
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
