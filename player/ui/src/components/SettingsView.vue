<script setup lang="ts">
import { RefreshCw } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";
import { checkServer, describeServer, type ServerCheck } from "../api";
import { artworkCacheStats, clearArtworkCache, dopStatus, inTauri, revealLogs, setArtworkCacheMaxBytes, setDsdDeviceConfirmed, type ArtworkCacheStats } from "../tauri";
import { useAnalogStore } from "../stores/analog";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import { useJobsStore } from "../stores/jobs";
import { useLibraryStore } from "../stores/library";
import { usePlaylistsStore } from "../stores/playlists";
import { useDeveloperStore } from "../stores/developer";
import { useSettingsStore } from "../stores/settings";
import {
  STREAM_FORMATS,
  type BitPerfectMode,
  type DsdStory,
  type StreamFormat,
} from "../types";
import DeviceCapabilities from "./DeviceCapabilities.vue";
import SignalPathPanel from "./SignalPathPanel.vue";
import SoundQualitySection from "./SoundQualitySection.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiBadge from "../ui/UiBadge.vue";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import UiSwitch from "../ui/UiSwitch.vue";
import ViewShell from "../ui/ViewShell.vue";
import AnalogSection from "./AnalogSection.vue";
import EqEditor from "./EqEditor.vue";
import CrossfeedSection from "./CrossfeedSection.vue";
import LimiterSection from "./LimiterSection.vue";
import SettingsSection from "./SettingsSection.vue";

const settings = useSettingsStore();
const developer = useDeveloperStore();
const lib = useLibraryStore();
const playlists = usePlaylistsStore();
const dsp = useDspStore();
const jobs = useJobsStore();

const urlInput = ref(settings.serverUrl);
const urlError = ref<string | null>(null);
const saving = ref(false);
const check = ref<ServerCheck | null>(null);
/** A Kahawai server answers (the light and the status line). */
const online = computed<boolean | null>(() =>
  check.value === null ? null : check.value.kind === "kahawai" || check.value.kind === "older",
);
/** What answers at the URL, in words. */
const connectionText = computed(() => {
  const c = check.value;
  if (c === null) return "Connection not checked";
  switch (c.kind) {
    case "kahawai":
      return `Server reachable · ${describeServer(c.identity)}`;
    case "older":
      return "Server reachable (an older Kahawai Server, without version details)";
    case "other":
      return "Something answers at this address, but it isn't a Kahawai server";
    default:
      return "Server unreachable";
  }
});
const connectionTitle = computed(() =>
  check.value?.kind === "kahawai"
    ? `Built ${check.value.identity.build.built_at} · library ${check.value.identity.catalog_id} · running since ${new Date(check.value.identity.started_at).toLocaleString()}`
    : undefined,
);

watch(
  () => settings.serverUrl,
  (u) => {
    urlInput.value = u;
  },
);

async function probe(): Promise<void> {
  check.value = await checkServer();
}

// Check as soon as Settings opens, so the light is meaningful without a click.
onMounted(() => void probe());

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
  { value: null, label: "Auto (recommended)" },
  ...STREAM_FORMATS.map((f) => ({ value: f, label: formatLabel(f) })),
];

const bitPerfectOptions: UiSelectOption[] = [
  { value: "auto", label: "Auto (follows Sound quality)" },
  { value: "off", label: "Off (shared output; EQ and volume work)" },
  { value: "mqa", label: "MQA files only" },
  { value: "all", label: "All tracks" },
];

const dsdOptions: UiSelectOption[] = [
  { value: "auto", label: "Auto (follows Sound quality)" },
  { value: "convert", label: "Convert to PCM (FLAC transcode)" },
  { value: "native", label: "Native DoP (needs a DSD-capable DAC)" },
];

/** Does the DSD setting effectively use native DoP for the current output? */
const dsdGoesNative = computed(
  () =>
    settings.dsdStory === "native" ||
    (settings.dsdStory === "auto" &&
      settings.qualityMode === "best" &&
      !!dsp.dop?.known_dsd_device &&
      !!dsp.dop?.capabilities?.external_dac),
);

/** A global format that would otherwise look like it governs DSD tracks. */
const formatIgnoredForDsd = computed(() => settings.globalFormat !== null && dsdGoesNative.value);

const player = usePlayerStore();
const analogStore = useAnalogStore();

/** Analog warmth is switched on (master and the active slot): the collapsed Experimental section says so. */
const analogOn = computed(() => analogStore.effective.enabled);

/** The Advanced section starts open only when something in it overrides the mode. */
const overrides = computed(() => [
  ...(settings.globalFormat !== null ? ["Stream format"] : []),
  ...(settings.bitPerfect !== "auto" ? ["Bit-perfect"] : []),
  ...(settings.dsdStory !== "auto" ? ["DSD handling"] : []),
]);
const advancedOpen = ref(overrides.value.length > 0);

/** Exclusive output is playing: your EQ, loudness and volume are bypassed. */
const bypassed = computed(() => player.isExclusive);

async function onConfirmDsdDevice(on: boolean): Promise<void> {
  await setDsdDeviceConfirmed(on);
  dsp.dop = (await dopStatus()) ?? dsp.dop;
}

// --- Album-art cache ---------------------------------------------------------

const artStats = ref<ArtworkCacheStats | null>(null);
const artClearing = ref(false);
const artResizing = ref(false);

async function refreshArtStats(): Promise<void> {
  artStats.value = (await artworkCacheStats()) ?? null;
}

/** KB under 1 MB, MB under 1 GB, GB above — matches how people think about disk space. */
function fmtBytes(n: number): string {
  if (n < 1024 * 1024) return `${Math.max(1, Math.round(n / 1024))} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

const sizeOptions = computed<UiSelectOption[]>(() =>
  (artStats.value?.size_options ?? []).map((b) => ({ value: String(b), label: fmtBytes(b) })),
);

async function onChangeMaxSize(v: string): Promise<void> {
  artResizing.value = true;
  try {
    artStats.value = (await setArtworkCacheMaxBytes(Number(v))) ?? artStats.value;
  } finally {
    artResizing.value = false;
  }
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
    // The jobs store reloads the library when the scan finishes.
    await jobs.startScan();
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
  ["A / B / X", "Analog warmth: listen to A, listen to B, switch"],
  ["1 … 6", "Albums / Artists / Playlists / Search / Queue / Settings"],
];

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
    <SignalPathPanel />

    <SettingsSection title="Server">
      <template #aside>
        <span
          class="text-[13px] leading-none"
          :class="online === true ? 'text-ok' : online === false ? 'text-danger' : 'text-faint'"
          role="img"
          :aria-label="online === true ? 'Connected' : online === false ? 'Offline' : 'Checking connection'"
          :title="online === true ? 'Connected' : online === false ? 'Offline' : 'Checking connection…'"
          :data-state="online === true ? 'connected' : online === false ? 'offline' : 'unknown'"
          data-testid="server-light"
        >●</span>
      </template>
      <UiHint>The music server this client browses.</UiHint>
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
        <span :title="connectionTitle" data-testid="connection-text">{{ connectionText }}</span>
      </p>
      <p
        v-if="check?.kind === 'kahawai' && check.identity.source_url"
        class="m-0 mb-2 select-text text-xs text-faint"
        data-testid="server-source"
      >
        Free software (GNU AGPL v3 or later). Source code: {{ check.identity.source_url }}
      </p>
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
      <DeviceCapabilities
        v-if="dsp.dop?.capabilities"
        :caps="dsp.dop.capabilities"
        :known-dsd="dsp.dop.known_dsd_device"
      />
      <div v-if="dsp.dop?.exclusive_available && dsp.dop.device" class="mb-2 flex items-center gap-2">
        <UiSwitch
          :model-value="dsp.dop.user_confirmed ?? false"
          :disabled="!!dsp.dop.known_dsd_device && !dsp.dop.user_confirmed"
          label="This output decodes DoP"
          data-testid="dsd-device-confirm"
          @update:model-value="(v) => onConfirmDsdDevice(v)"
        />
      </div>
      <UiHint v-if="dsp.outputDeviceMissing" tone="warn" data-testid="device-missing">
        “{{ dsp.outputDevice }}” is not connected, so the system default is being used. It is selected again
        automatically when it reappears (press the rescan button).
      </UiHint>
      <UiHint v-if="!dsp.devices.length">No output devices reported.</UiHint>
      <UiHint>
        PCM plays in shared mode. On macOS, DSD tracks can use exclusive DoP output instead — bit-perfect,
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

    <SoundQualitySection />

    <SettingsSection title="Crossfeed">
      <CrossfeedSection />
    </SettingsSection>

    <SettingsSection title="Parametric EQ">
      <EqEditor
        unsupported-note="Bypassed while exclusive output is playing. Switch Sound quality to Compatible to use it."
      >
        <template #note>
          <UiHint>
            Up to 12 bands, applied to PCM only (DoP bypasses EQ). Drag a point to shape the sound,
            double-click the graph to add a band. Changes apply live and are saved as you go.
          </UiHint>
        </template>
      </EqEditor>
    </SettingsSection>

    <SettingsSection title="Loudness normalization">
      <p v-if="bypassed" class="m-0 mb-2 text-sm text-dim" role="status" data-testid="loudness-bypassed">
        Bypassed while exclusive output is playing. Switch Sound quality to Compatible to use it.
      </p>
      <div :class="bypassed ? 'pointer-events-none opacity-40 grayscale' : ''" :inert="bypassed || undefined">
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
      </div>
    </SettingsSection>

    <SettingsSection title="Limiter">
      <LimiterSection />
    </SettingsSection>

    <details class="mb-7 rounded-lg border border-line" data-testid="advanced" :open="advancedOpen" @toggle="advancedOpen = ($event.target as HTMLDetailsElement).open">
      <summary class="heading-3 cursor-pointer select-none px-3 py-2">
        Advanced
        <UiBadge v-if="overrides.length" variant="accent" class="ml-1" data-testid="advanced-overrides">
          {{ overrides.length }} override{{ overrides.length === 1 ? "" : "s" }}
        </UiBadge>
      </summary>
      <div class="px-3 pt-1">
        <UiHint>
          Fine controls. Each defaults to Auto, which follows Sound quality above; choose a value here only to override it.
        </UiHint>
    <SettingsSection title="Stream format">
      <UiHint>
        How music is sent to this player. <strong class="font-semibold text-fg">Auto</strong> is best for most people: it plays
        each file as-is when it can, and converts only when it has to. Pick a format here to use it for
        <em>every</em> track. To change just one track, use the format picker in the player bar at the bottom.
      </UiHint>
      <UiSelect
        aria-label="Default stream format"
        :model-value="settings.globalFormat"
        :options="formatOptions"
        @update:model-value="(v) => settings.saveGlobalFormat(v as StreamFormat | null)"
      />
    </SettingsSection>

    <SettingsSection title="Bit-perfect output">
      <template v-if="dsp.dop?.exclusive_available">
        <UiHint>
          Sends a file's samples to your DAC <strong class="font-semibold text-fg">untouched</strong>, at the file's own
          sample rate, with exclusive control of the device. EQ, loudness, volume and format conversion are bypassed.
          This is what a DAC that decodes <strong class="font-semibold text-fg">MQA</strong> needs to see the MQA signal.
        </UiHint>
        <UiSelect
          aria-label="Bit-perfect output"
          :model-value="settings.bitPerfect"
          :options="bitPerfectOptions"
          @update:model-value="(v) => settings.saveBitPerfect((v ?? 'off') as BitPerfectMode)"
        />
        <UiHint v-if="settings.bitPerfect === 'mqa' || settings.bitPerfect === 'all'" spaced data-testid="bit-perfect-notes">
          Use your DAC's own volume control. Other apps can't play through the device while a track is playing, and
          tracks play one at a time, with a brief gap between them. If the device can't take a file's sample rate, that
          track plays normally instead.
        </UiHint>
      </template>
      <UiHint v-else>Bit-perfect output is macOS-only.</UiHint>
    </SettingsSection>

    <SettingsSection title="DSD handling">
      <UiHint>
        What to do with DSD tracks (DSF/DFF). <strong class="font-semibold text-fg">Auto</strong> plays them natively
        (DoP) on DACs known to decode it and converts to FLAC everywhere else, so an unfamiliar device never gets a
        DoP stream it might play as noise.
      </UiHint>
      <UiSelect
        aria-label="DSD handling"
        :model-value="settings.dsdStory"
        :options="dsdOptions"
        @update:model-value="(v) => settings.saveDsdStory(v as DsdStory)"
      />
      <UiHint v-if="settings.dsdStory === 'auto' && dsp.dop?.device" data-testid="dsd-auto-note">
        Output: “{{ dsp.dop.device.trim() }}” —
        <template v-if="settings.qualityMode === 'compatible'">Compatible mode converts DSD to FLAC.</template>
        <template v-else-if="dsp.dop.known_dsd_device">
          {{ dsp.dop.user_confirmed ? "confirmed by you" : "a known DSD DAC" }}, so DSD plays natively (unless your
          EQ, loudness or volume is on).
        </template>
        <template v-else>
          not a known DSD DAC, so DSD is converted to FLAC. If it decodes DoP, confirm it under Audio output.
        </template>
      </UiHint>
      <UiHint v-if="formatIgnoredForDsd" tone="warn" data-testid="dsd-format-ignored">
        The Stream format above is set, but it does not apply to DSD tracks while DSD plays natively. It still applies to
        every other track.
      </UiHint>
      <UiHint spaced>
        Native plays DSD as DoP; on macOS it uses exclusive output (bit-perfect, bypasses EQ/loudness/volume). If the
        device can't play DoP, the track plays as FLAC instead and the player says why.
      </UiHint>
    </SettingsSection>
      </div>
    </details>

    <details class="mb-7 rounded-lg border border-line" data-testid="experimental">
      <summary class="heading-3 cursor-pointer select-none px-3 py-2">
        Experimental
        <UiBadge v-if="analogOn" variant="accent" class="ml-1" data-testid="experimental-active">Analog warmth on</UiBadge>
      </summary>
      <div class="px-3 pt-1">
        <UiHint>
          Work in progress: these may change or be removed, and are off unless you turn them on.
        </UiHint>
        <AnalogSection />
      </div>
    </details>

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
          <div class="h-1.5 max-w-[200px] flex-[2] overflow-hidden rounded-sm bg-active">
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
        the server is offline. Older ones are dropped automatically once the cache passes its limit below.
      </UiHint>
      <UiHint v-if="artStats">
        {{ artStats.files }} {{ artStats.files === 1 ? "image" : "images" }} · {{ fmtBytes(artStats.bytes) }} of
        {{ fmtBytes(artStats.max_bytes) }}
      </UiHint>
      <div class="flex flex-wrap items-center gap-3">
        <label class="flex items-center gap-2 text-dim">
          Limit
          <UiSelect
            aria-label="Album art cache limit"
            trigger-class="w-[90px]"
            :model-value="String(artStats?.max_bytes ?? '')"
            :options="sizeOptions"
            :disabled="artResizing || !artStats"
            data-testid="artwork-cache-limit"
            @update:model-value="(v) => v && onChangeMaxSize(v)"
          />
        </label>
        <span v-if="artStats?.free_bytes != null" class="text-xs text-dim" data-testid="artwork-cache-free">
          {{ fmtBytes(artStats.free_bytes) }} free on disk
        </span>
      </div>
      <UiButton variant="icon" :disabled="artClearing || !artStats?.files" @click="onClearArt">
        {{ artClearing ? "Clearing…" : "Clear cache" }}
      </UiButton>
    </SettingsSection>

    <SettingsSection title="Keyboard shortcuts">
      <UiHint>Available everywhere except while typing in a text field.</UiHint>
      <ul class="m-0 grid list-none grid-cols-1 gap-x-4 gap-y-1.5 p-0 min-[560px]:grid-cols-2">
        <li v-for="[key, desc] in KEYBOARD_MAP" :key="key" class="flex items-center gap-2.5 text-dim">
          <kbd class="min-w-14 rounded-sm border border-line bg-surface px-1.5 py-0.5 text-center font-mono text-[11px] text-fg">{{ key }}</kbd>
          <span>{{ desc }}</span>
        </li>
      </ul>
    </SettingsSection>

    <SettingsSection v-if="inTauri()" title="Logs">
      <UiHint>
        If something goes wrong, the log shows what happened. Attach it (kahawai-player.log) when you
        report a problem.
      </UiHint>
      <UiButton data-testid="reveal-logs" @click="revealLogs()">Reveal Logs in Finder</UiButton>
    </SettingsSection>

    <SettingsSection v-if="inTauri()" title="Developer tools">
      <UiHint>
        For troubleshooting. Right-clicking where the app has no menu of its own then shows WebKit's menu
        (Reload, Inspect Element), and the Web Inspector is available. Leave it off otherwise.
      </UiHint>
      <p v-if="developer.devBuild" class="m-0 text-xs text-dim" data-testid="developer-dev-build">
        This is a development build: they're always on here.
      </p>
      <template v-else>
        <UiSwitch
          :model-value="developer.enabled"
          label="Developer tools"
          data-testid="developer-toggle"
          @update:model-value="(v) => developer.set(v)"
        />
        <p
          v-if="developer.enabled !== developer.inspector"
          class="m-0 mt-2 text-xs text-dim"
          role="status"
          data-testid="developer-restart"
        >
          {{
            developer.enabled
              ? "WebKit's menu is on now. Restart the app for the Web Inspector."
              : "WebKit's menu is off now. The Web Inspector goes away when you restart the app."
          }}
        </p>
      </template>
    </SettingsSection>
  </ViewShell>
</template>
