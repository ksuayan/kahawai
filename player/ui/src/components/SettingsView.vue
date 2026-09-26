<script setup lang="ts">
import { Plus, RefreshCw, X } from "lucide-vue-next";
import { computed, ref, watch } from "vue";
import { checkHealth } from "../api";
import { artworkCacheStats, clearArtworkCache, inTauri, type ArtworkCacheStats } from "../tauri";
import { useDspStore } from "../stores/dsp";
import { useJobsStore } from "../stores/jobs";
import { useLibraryStore } from "../stores/library";
import { usePlaylistsStore } from "../stores/playlists";
import { useSettingsStore } from "../stores/settings";
import {
  EQ_BAND_TYPES,
  STREAM_FORMATS,
  type DsdStory,
  type EqBandType,
  type StreamFormat,
} from "../types";
import StateMessage from "../ui/StateMessage.vue";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import UiSwitch from "../ui/UiSwitch.vue";
import ViewShell from "../ui/ViewShell.vue";
import SettingsSection from "./SettingsSection.vue";

const settings = useSettingsStore();
const lib = useLibraryStore();
const playlists = usePlaylistsStore();
const dsp = useDspStore();
const jobs = useJobsStore();

const urlInput = ref(settings.serverUrl);
const urlError = ref<string | null>(null);
const saving = ref(false);
const online = ref<boolean | null>(null);

watch(
  () => settings.serverUrl,
  (u) => {
    urlInput.value = u;
  },
);

async function probe(): Promise<void> {
  online.value = await checkHealth();
}

async function save(): Promise<void> {
  const url = urlInput.value.trim();
  if (!url) {
    urlError.value = "URL cannot be empty.";
    return;
  }
  saving.value = true;
  urlError.value = null;
  try {
    await settings.saveServerUrl(url);
    await probe();
    // Re-fetch the whole library against the new server.
    await Promise.all([lib.loadAll(), playlists.load()]);
  } catch (e) {
    urlError.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}

const defaultDeviceName = computed(() => dsp.devices.find((d) => d.is_default)?.name ?? "");

// Device names are passed through exactly (never trimmed): some end in a space.
const deviceOptions = computed<UiSelectOption[]>(() => [
  { value: null, label: `System default${defaultDeviceName.value ? ` (${defaultDeviceName.value})` : ""}` },
  ...(dsp.outputDeviceMissing && dsp.outputDevice !== null
    ? [{ value: dsp.outputDevice, label: `${dsp.outputDevice} — not connected` }]
    : []),
  ...dsp.devices.map((d) => ({ value: d.name, label: d.name })),
]);

function formatLabel(f: StreamFormat): string {
  return f === "passthrough" ? "Passthrough (original)" : f.toUpperCase();
}

const formatOptions: UiSelectOption[] = [
  { value: null, label: "Auto (server default)" },
  ...STREAM_FORMATS.map((f) => ({ value: f, label: formatLabel(f) })),
];

const dsdOptions: UiSelectOption[] = [
  { value: "convert", label: "Convert to PCM (FLAC transcode)" },
  { value: "native", label: "Native DoP (needs a DSD-capable DAC)" },
];

// --- Album-art cache ---------------------------------------------------------

const artStats = ref<ArtworkCacheStats | null>(null);
const artClearing = ref(false);

async function refreshArtStats(): Promise<void> {
  artStats.value = (await artworkCacheStats()) ?? null;
}

function fmtBytes(n: number): string {
  if (n < 1024 * 1024) return `${Math.max(1, Math.round(n / 1024))} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

async function onClearArt(): Promise<void> {
  artClearing.value = true;
  try {
    await clearArtworkCache();
    await refreshArtStats();
    // Re-request covers so the grid refills from the server right away.
    await lib.loadAll();
  } finally {
    artClearing.value = false;
  }
}

if (inTauri()) void refreshArtStats();

// --- Library scan + jobs -----------------------------------------------------

const scanning = ref(false);

async function onScan(): Promise<void> {
  scanning.value = true;
  try {
    await jobs.startScan();
    // A finished scan changes the catalog: reload browse data.
    // (Poll for completion is overkill; the user sees the toast.)
  } finally {
    scanning.value = false;
  }
}

const KEYBOARD_MAP: [string, string][] = [
  ["Space", "Play / pause"],
  ["← / →", "Seek ∓ 10 seconds"],
  ["↑ / ↓", "Volume up / down"],
  ["N / P", "Next / previous track"],
  ["F", "Go to search"],
  ["1 … 6", "Albums / Artists / Playlists / Search / Queue / Settings"],
];

// --- EQ ---------------------------------------------------------------------

function bandTypeLabel(t: EqBandType): string {
  return t
    .split("_")
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

const bandTypeOptions: UiSelectOption[] = EQ_BAND_TYPES.map((t) => ({ value: t, label: bandTypeLabel(t) }));

function onBandNum(i: number, field: "freq" | "gain_db" | "q", e: Event): void {
  const v = Number((e.target as HTMLInputElement).value);
  dsp.updateRow(i, { [field]: v });
}

// --- Loudness ----------------------------------------------------------------

const loudnessInput = ref("-14");
const loudnessError = ref<string | null>(null);

watch(
  () => dsp.loudnessTarget,
  (v) => {
    loudnessInput.value = String(v);
  },
  { immediate: true },
);

async function onLoudnessTarget(): Promise<void> {
  const v = Number(loudnessInput.value);
  const ok = await dsp.saveLoudnessTarget(v);
  loudnessError.value = ok ? null : "Target must be −40…−1 LUFS.";
  if (ok) loudnessInput.value = String(dsp.loudnessTarget);
}

// --- Output ------------------------------------------------------------------

const dopRates = computed(() =>
  (dsp.dop?.supported_rates ?? []).map((r) => `${Math.round(r / 100) / 10} kHz`).join(", "),
);
</script>

<template>
  <ViewShell title="Settings" width="narrow">
    <SettingsSection title="Server">
      <UiHint>The music server this client browses. Saved through the Rust core.</UiHint>
      <div class="flex gap-2">
        <UiInput
          v-model="urlInput"
          class="flex-1"
          type="text"
          spellcheck="false"
          placeholder="http://localhost:8080"
          aria-label="Server URL"
          @keydown.enter="save"
        />
        <UiButton variant="primary" :disabled="saving" @click="save">{{ saving ? "Saving…" : "Save" }}</UiButton>
      </div>
      <StateMessage v-if="urlError" kind="error" class="mt-3">{{ urlError }}</StateMessage>
      <p
        class="my-2 flex items-center gap-1.5 text-xs"
        :class="online === true ? 'text-ok' : online === false ? 'text-danger' : 'text-dim'"
        data-testid="connection-status"
      >
        <UiButton variant="icon" title="Check connection" aria-label="Check connection" @click="probe"><RefreshCw /></UiButton>
        {{ online === null ? "Connection not checked" : online ? "Server reachable" : "Server unreachable" }}
      </p>
    </SettingsSection>

    <SettingsSection title="Default stream format">
      <UiHint>
        Global preference sent via <code class="font-mono text-xs">set_format</code>. Per-track overrides in the
        now-playing bar take precedence for that track.
      </UiHint>
      <UiSelect
        aria-label="Default stream format"
        :model-value="settings.globalFormat"
        :options="formatOptions"
        @update:model-value="(v) => settings.saveGlobalFormat(v as StreamFormat | null)"
      />
    </SettingsSection>

    <SettingsSection title="DSD handling">
      <UiHint>
        What to do with DSD tracks (DSF/DFF) when no explicit format override applies. Saved through the Rust core
        and restored on launch.
      </UiHint>
      <UiSelect
        aria-label="DSD handling"
        :model-value="settings.dsdStory"
        :options="dsdOptions"
        @update:model-value="(v) => settings.saveDsdStory(v as DsdStory)"
      />
      <UiHint spaced>
        Native requests DoP from the server; on macOS it plays through the exclusive hog-mode path (bit-perfect,
        bypasses EQ/loudness/volume). Without a DoP-capable device the core falls back to the FLAC transcode and logs
        why.
      </UiHint>
    </SettingsSection>

    <SettingsSection title="Library">
      <UiHint>
        Rescan the server's music roots. Progress and completion are reported as toasts; a second scan while one is
        running is a no-op (“Scan already running”).
      </UiHint>
      <UiButton variant="primary" :disabled="scanning || jobs.hasActive" @click="onScan">
        {{ jobs.hasActive ? "Working…" : "Scan library" }}
      </UiButton>
      <div v-if="jobs.activeJobs.length > 0" class="mt-3 flex flex-col gap-2">
        <div v-for="j in jobs.activeJobs" :key="j.id" class="flex items-center gap-2.5 text-[13px]" data-testid="active-job">
          <span class="min-w-0 flex-1 truncate">{{ j.kind === "scan" ? "Library scan" : j.label }}</span>
          <div class="h-1.5 max-w-[200px] flex-[2] overflow-hidden rounded-[3px] bg-active">
            <div class="h-full bg-accent transition-[width] duration-[400ms] ease-linear" :style="{ width: `${Math.round(j.progress * 100)}%` }" />
          </div>
          <span class="min-w-9 text-right text-xs tabular-nums text-dim">{{ Math.round(j.progress * 100) }}%</span>
        </div>
      </div>
      <div v-else-if="jobs.jobs.length > 0" class="mt-3 flex flex-col gap-2">
        <UiHint>Recent jobs:</UiHint>
        <div v-for="j in jobs.jobs.slice(0, 5)" :key="j.id" class="flex items-center gap-2.5 text-[13px] text-dim" data-testid="recent-job">
          <span class="min-w-0 flex-1 truncate">{{ j.kind === "scan" ? "Library scan" : j.label }}</span>
          <UiBadge :variant="j.status === 'done' ? 'ok' : 'bad'">{{ j.status }}</UiBadge>
        </div>
        <div><UiButton variant="icon" @click="jobs.refresh()"><RefreshCw /> Refresh</UiButton></div>
      </div>
    </SettingsSection>

    <SettingsSection v-if="inTauri()" title="Album art cache">
      <UiHint>
        Covers are saved on disk the first time they are shown, so they load instantly afterwards and still appear if
        the server is offline. Older ones are dropped automatically once the cache passes 512&nbsp;MB.
      </UiHint>
      <UiHint v-if="artStats">
        {{ artStats.files }} {{ artStats.files === 1 ? "image" : "images" }} · {{ fmtBytes(artStats.bytes) }}
      </UiHint>
      <UiButton variant="icon" :disabled="artClearing || !artStats?.files" @click="onClearArt">
        {{ artClearing ? "Clearing…" : "Clear cache" }}
      </UiButton>
    </SettingsSection>

    <SettingsSection title="Audio output">
      <UiHint>
        Choose the speaker or DAC to play through. Changing it while music is playing moves the track over and
        continues from the same position. DSD played through exclusive DoP uses the same device.
      </UiHint>
      <div class="flex gap-2">
        <UiSelect
          aria-label="Output device"
          trigger-class="flex-1"
          :model-value="dsp.outputDevice"
          :options="deviceOptions"
          @update:model-value="(v) => dsp.chooseOutputDevice(v)"
        />
        <UiButton variant="icon" title="Rescan output devices" aria-label="Rescan output devices" @click="dsp.refreshDevices()"><RefreshCw /></UiButton>
      </div>
      <UiHint v-if="dsp.outputDeviceMissing" tone="warn" data-testid="device-missing">
        “{{ dsp.outputDevice }}” is not connected, so the system default is being used. It is selected again
        automatically when it reappears (press the rescan button).
      </UiHint>
      <UiHint v-if="!dsp.devices.length">No output devices reported.</UiHint>
      <UiHint>
        PCM plays in shared mode. On macOS, DSD tracks can use exclusive hog-mode DoP output instead — bit-perfect,
        bypassing the EQ, loudness, and volume below.
      </UiHint>
      <UiHint>
        <template v-if="dsp.dop?.exclusive_available">
          Exclusive DoP is available on this Mac.
          <template v-if="dopRates">Device accepts DoP at {{ dopRates }}.</template>
          <template v-else>
            The current output device reports no DoP-capable rate — DSD falls back to the FLAC transcode and the
            reason is logged.
          </template>
        </template>
        <template v-else>Exclusive DoP output is macOS-only.</template>
      </UiHint>
    </SettingsSection>

    <SettingsSection title="Parametric EQ">
      <UiHint>
        Up to 8 bands, applied to PCM only (DoP bypasses EQ). Changes apply live and are saved through the Rust core.
      </UiHint>
      <UiSwitch :model-value="dsp.eqEnabled" label="EQ enabled" @update:model-value="(v) => dsp.saveEqEnabled(v)" />
      <StateMessage v-if="dsp.rowError" kind="error" class="mt-2">{{ dsp.rowError }}</StateMessage>
      <div class="my-2.5 flex flex-col gap-1.5">
        <div
          v-for="(b, i) in dsp.rows"
          :key="i"
          class="flex flex-wrap items-center gap-2 rounded-lg border border-line bg-raised px-2.5 py-2"
          :class="!b.enabled && 'opacity-50'"
          data-testid="eq-band"
        >
          <UiSwitch :model-value="b.enabled" aria-label="Enable this band" @update:model-value="dsp.toggleRow(i)" />
          <UiSelect
            aria-label="Band type"
            trigger-class="min-w-[130px]"
            :model-value="b.band_type"
            :options="bandTypeOptions"
            @update:model-value="(v) => dsp.updateRow(i, { band_type: v as EqBandType })"
          />
          <label class="flex items-center gap-1 text-dim" title="Frequency (Hz)">
            Hz
            <UiInput class="w-[76px]" type="number" :model-value="String(b.freq)" min="10" max="24000" step="1" @change="onBandNum(i, 'freq', $event)" />
          </label>
          <label class="flex items-center gap-1 text-dim" title="Gain (dB)">
            dB
            <UiInput class="w-[64px]" type="number" :model-value="String(b.gain_db)" min="-24" max="24" step="0.5" @change="onBandNum(i, 'gain_db', $event)" />
          </label>
          <label class="flex items-center gap-1 text-dim" title="Q (shelf slope for shelves)">
            Q
            <UiInput class="w-[60px]" type="number" :model-value="String(b.q)" min="0.1" max="18" step="0.1" @change="onBandNum(i, 'q', $event)" />
          </label>
          <UiButton variant="icon-danger" title="Remove band" aria-label="Remove band" @click="dsp.removeBand(i)"><X /></UiButton>
        </div>
      </div>
      <UiButton variant="icon" :disabled="!dsp.canAddBand" @click="dsp.addBand()"><Plus /> Add band</UiButton>
    </SettingsSection>

    <SettingsSection title="Loudness normalization">
      <UiHint>
        EBU R128-style loudness matching for PCM only (DoP bypasses it). The first play of each track does a fast
        pre-scan — one extra stream, roughly double the bandwidth — then the measured gain is cached.
      </UiHint>
      <UiSwitch
        :model-value="dsp.loudnessEnabled"
        label="Loudness normalization enabled"
        @update:model-value="(v) => dsp.saveLoudnessEnabled(v)"
      />
      <div class="mt-2.5 flex">
        <label class="flex items-center gap-2 text-dim">
          Target
          <UiInput v-model="loudnessInput" class="w-[76px]" type="number" min="-40" max="-1" step="0.5" @change="onLoudnessTarget" />
          LUFS
        </label>
      </div>
      <StateMessage v-if="loudnessError" kind="error" class="mt-2">{{ loudnessError }}</StateMessage>
    </SettingsSection>

    <SettingsSection title="Keyboard shortcuts">
      <UiHint>Available everywhere except while typing in a text field.</UiHint>
      <ul class="m-0 grid list-none grid-cols-1 gap-x-4 gap-y-1.5 p-0 min-[560px]:grid-cols-2">
        <li v-for="[key, desc] in KEYBOARD_MAP" :key="key" class="flex items-center gap-2.5 text-dim">
          <kbd class="min-w-14 rounded border border-line bg-raised px-1.5 py-0.5 text-center font-mono text-[11px] text-fg">{{ key }}</kbd>
          <span>{{ desc }}</span>
        </li>
      </ul>
    </SettingsSection>
  </ViewShell>
</template>
