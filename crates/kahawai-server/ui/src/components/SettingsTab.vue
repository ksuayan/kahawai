<script setup lang="ts">
import { X } from "lucide-vue-next";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import { computed, onMounted } from "vue";
import { useEnrichmentStore } from "../stores/enrichment";
import { setupRevealLogs } from "../tauri";
import { useSetupStore } from "../stores/setup";
import { bookDirChipClass, bookDirChipText, CONFIDENCE_LEVELS, dirChipClass, dirChipText } from "../types";

const setup = useSetupStore();
const enrich = useEnrichmentStore();
onMounted(() => {
  void enrich.load();
  // Opened before the running server's folders were read (it was still
  // starting): read them now.
  if (setup.runningDirs.length === 0 && setup.pendingRemoves.length === 0) void setup.loadRunningConfig();
  void setup.loadAudiobookRoots();
});

const n = (v: number) => v.toLocaleString("en-US");

/** The presets, plus a hand-edited threshold from the config file. */
const levels = computed(() => {
  const current = enrich.status?.min_confidence;
  if (current === undefined || CONFIDENCE_LEVELS.some((l) => l.value === current)) {
    return CONFIDENCE_LEVELS;
  }
  return [...CONFIDENCE_LEVELS, { value: current, label: `Custom (${Math.round(current * 100)}%)` }];
});

const levelOptions = computed<UiSelectOption[]>(() =>
  levels.value.map((l) => ({ value: String(l.value), label: l.label })),
);

/** Nothing waiting, but some albums weren't found: they can be tried
 *  again (lowering the strictness does this by itself). */
const retryable = computed(() =>
  enrich.status && enrich.status.coverage.pending_lookup === 0 ? enrich.status.coverage.no_match : 0,
);

/** One line for the latest lookup; empty when there's none. */
const jobLine = computed(() => {
  const j = enrich.job;
  if (!j) return "";
  switch (j.status) {
    case "queued":
      return "Lookup queued…";
    case "running":
      return `Looking up… ${Math.round(j.progress * 100)}%`;
    case "paused":
      return j.message ?? "Lookup paused.";
    case "cancelled":
      return "Last lookup cancelled.";
    case "failed":
      return `Last lookup failed: ${j.message ?? "unknown error"}`;
    default:
      return j.message ? `Last lookup: ${j.message}` : "Last lookup finished.";
  }
});
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

    <h3 class="heading-3 mb-2 mt-4">Audiobook folders</h3>
    <UiHint tone="faint">
      Kept apart from the music, with their own library in the Player. Each folder of audio files is one
      book.
    </UiHint>
    <ul class="mb-3 flex flex-col gap-2" data-testid="audiobook-roots">
      <li
        v-for="d in setup.runningBookDirs"
        :key="d.path"
        class="flex items-center justify-between gap-3 rounded-md border border-line bg-raised px-3 py-2"
        data-testid="audiobook-row"
      >
        <div class="min-w-0">
          <div class="flex items-center gap-2 truncate text-[13px]">
            {{ d.path }}
            <span
              v-if="setup.pendingBookAdds.includes(d.path)"
              class="micro-label rounded-sm bg-accent/15 px-1 py-0.5 text-accent"
            >
              new
            </span>
          </div>
          <div
            class="text-xs"
            :class="d.validation ? bookDirChipClass(d.validation) : 'text-faint'"
            :title="d.validation?.truncated ? 'This is a lower bound — the folder has more than this quick check counts. The actual scan is never capped.' : undefined"
            data-testid="audiobook-chip"
          >
            {{ d.validating ? "checking…" : d.validation ? bookDirChipText(d.validation) : "" }}
          </div>
        </div>
        <UiButton
          variant="icon-danger"
          aria-label="Remove audiobook folder"
          data-testid="remove-audiobook-folder"
          @click="setup.removeRunningAudiobook(d.path)"
        >
          <X class="size-4" />
        </UiButton>
      </li>
    </ul>
    <p v-if="setup.runningBookDirs.length === 0" class="mb-2 text-xs text-faint">No audiobook folders.</p>
    <div class="mb-2 flex flex-wrap items-center gap-2">
      <UiButton data-testid="add-audiobook-folder" @click="setup.addRunningAudiobookFromPicker()">Add folder…</UiButton>
      <UiButton
        variant="primary"
        :disabled="!setup.canApplyBooks || setup.applyingBooks"
        data-testid="apply-audiobooks"
        @click="setup.applyAudiobooks()"
      >
        {{ setup.applyingBooks || (setup.isScanning && !setup.canApplyBooks) ? "Scanning…" : "Apply" }}
      </UiButton>
    </div>
    <UiHint v-if="setup.canApplyBooks" tone="faint">
      {{ setup.pendingBookAdds.length }} to add, {{ setup.pendingBookRemoves.length }} to remove — not applied
      until you click Apply. Removing a folder forgets its books and your progress; the files stay.
    </UiHint>
    <UiHint v-if="setup.audiobookError" tone="warn">{{ setup.audiobookError }}</UiHint>

    <h3 class="heading-3 mb-2 mt-4">Album info</h3>
    <label class="mb-1 flex items-center gap-2 text-[13px]">
      <input
        type="checkbox"
        :checked="enrich.status?.enabled ?? false"
        :disabled="!enrich.status || enrich.busy"
        @change="enrich.setEnabled(($event.target as HTMLInputElement).checked)"
      />
      Look up missing album info online (MusicBrainz, Cover Art Archive)
    </label>
    <UiHint tone="faint">
      Sends album and artist names to musicbrainz.org, one request a second at most. Only fills
      in what's missing (release ID, year, cover); your tags are never changed.
    </UiHint>
    <div v-if="enrich.status" class="mt-2 flex flex-wrap items-center gap-2 text-[13px]">
      <span aria-hidden="true">Match strictness</span>
      <UiSelect
        aria-label="Match strictness"
        trigger-class="w-44"
        :model-value="String(enrich.status.min_confidence)"
        :options="levelOptions"
        :disabled="enrich.busy"
        @update:model-value="(v) => v !== null && enrich.setThreshold(Number(v))"
      />
    </div>
    <UiHint v-if="enrich.status" tone="faint">
      Lowering it looks up the albums that weren't found again, from the saved MusicBrainz replies (no
      new requests).
    </UiHint>
    <UiHint v-if="enrich.status" tone="faint">
      {{ n(enrich.status.coverage.total_albums) }} albums ·
      {{ n(enrich.status.coverage.with_embedded_mbid) }} identified by their tags ·
      {{ n(enrich.status.coverage.matched_online) }} found online ·
      {{ n(enrich.status.coverage.no_match) }} not found ·
      {{ n(enrich.status.coverage.pending_lookup) }} waiting
    </UiHint>
    <!-- Fixed height: the line changes as a lookup runs, the layout doesn't. -->
    <p
      class="mt-1 min-h-[2.5rem] text-xs"
      :class="enrich.paused || enrich.job?.status === 'failed' ? 'text-warn-fg' : 'text-dim'"
      aria-live="polite"
    >
      {{ jobLine }}
    </p>
    <div v-if="enrich.status" class="flex flex-wrap gap-2">
      <UiButton
        v-if="!enrich.running && !enrich.paused && retryable === 0"
        :disabled="!enrich.status.enabled || enrich.busy || enrich.status.coverage.pending_lookup === 0"
        @click="enrich.act('start')"
      >
        Look up now
      </UiButton>
      <UiButton
        v-if="!enrich.running && !enrich.paused && retryable > 0"
        :disabled="!enrich.status.enabled || enrich.busy"
        title="Look up the albums that weren't found again, with the current match strictness"
        @click="enrich.act('retry')"
      >
        Retry not found ({{ n(retryable) }})
      </UiButton>
      <UiButton v-if="enrich.running" :disabled="enrich.busy" @click="enrich.act('pause')">Pause</UiButton>
      <UiButton v-if="enrich.paused" variant="primary" :disabled="enrich.busy" @click="enrich.act('resume')">
        Resume
      </UiButton>
      <UiButton v-if="enrich.running || enrich.paused" variant="danger" :disabled="enrich.busy" @click="enrich.act('cancel')">
        Cancel
      </UiButton>
    </div>
    <UiHint v-if="enrich.error" tone="warn">{{ enrich.error }}</UiHint>

    <h3 class="heading-3 mb-2 mt-4">Advanced</h3>
    <UiHint tone="faint">
      Database: {{ setup.runningDbPath }} · Bind address: {{ setup.runningBind }}
    </UiHint>
    <div class="mt-2 flex flex-wrap gap-2">
      <UiButton @click="setup.editConfiguration()">Change bind/database (restart required)</UiButton>
      <UiButton @click="setup.revealConfig()">Reveal Config in Finder</UiButton>
      <UiButton data-testid="reveal-logs" @click="setupRevealLogs()">Reveal Logs in Finder</UiButton>
    </div>
  </div>
</template>
