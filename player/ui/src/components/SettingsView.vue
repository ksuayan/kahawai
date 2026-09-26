<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { checkHealth } from "../api";
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

async function onFormatChange(e: Event): Promise<void> {
  const v = (e.target as HTMLSelectElement).value;
  await settings.saveGlobalFormat(v === "" ? null : (v as StreamFormat));
}

function formatLabel(f: StreamFormat): string {
  return f === "passthrough" ? "Passthrough (original)" : f.toUpperCase();
}

function onDsdStory(e: Event): void {
  void settings.saveDsdStory((e.target as HTMLSelectElement).value as DsdStory);
}

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

function onBandType(i: number, e: Event): void {
  dsp.updateRow(i, { band_type: (e.target as HTMLSelectElement).value as EqBandType });
}

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
  <div class="view settings">
    <h2>Settings</h2>

    <section>
      <h3>Server</h3>
      <p class="hint">The music server this client browses. Saved through the Rust core.</p>
      <div class="row">
        <input
          v-model="urlInput"
          type="text"
          spellcheck="false"
          placeholder="http://localhost:8080"
          @keydown.enter="save"
        />
        <button class="primary" :disabled="saving" @click="save">
          {{ saving ? "Saving…" : "Save" }}
        </button>
      </div>
      <div v-if="urlError" class="error-banner">{{ urlError }}</div>
      <p class="status" :class="{ ok: online === true, bad: online === false }">
        <button class="icon-btn" title="Check connection" @click="probe">⟳</button>
        {{
          online === null ? "Connection not checked" : online ? "Server reachable" : "Server unreachable"
        }}
      </p>
    </section>

    <section>
      <h3>Default stream format</h3>
      <p class="hint">
        Global preference sent via <code>set_format</code>. Per-track overrides in the
        now-playing bar take precedence for that track.
      </p>
      <select :value="settings.globalFormat ?? ''" @change="onFormatChange">
        <option value="">Auto (server default)</option>
        <option v-for="f in STREAM_FORMATS" :key="f" :value="f">{{ formatLabel(f) }}</option>
      </select>
    </section>

    <section>
      <h3>DSD handling</h3>
      <p class="hint">
        What to do with DSD tracks (DSF/DFF) when no explicit format override
        applies. Saved through the Rust core and restored on launch.
      </p>
      <select :value="settings.dsdStory" @change="onDsdStory">
        <option value="convert">Convert to PCM (FLAC transcode)</option>
        <option value="native">Native DoP (needs a DSD-capable DAC)</option>
      </select>
      <p class="hint">
        Native requests DoP from the server; on macOS it plays through the
        exclusive hog-mode path (bit-perfect, bypasses EQ/loudness/volume).
        Without a DoP-capable device the core falls back to the FLAC transcode
        and logs why.
      </p>
    </section>

    <section>
      <h3>Library</h3>
      <p class="hint">
        Rescan the server's music roots. Progress and completion are reported
        as toasts; a second scan while one is running is a no-op
        (“Scan already running”).
      </p>
      <button class="primary" :disabled="scanning || jobs.hasActive" @click="onScan">
        {{ jobs.hasActive ? "Working…" : "Scan library" }}
      </button>
      <div v-if="jobs.activeJobs.length > 0" class="jobs">
        <div v-for="j in jobs.activeJobs" :key="j.id" class="job">
          <span class="job-label">{{ j.kind === "scan" ? "Library scan" : j.label }}</span>
          <div class="job-bar"><div class="job-fill" :style="{ width: `${Math.round(j.progress * 100)}%` }" /></div>
          <span class="job-pct">{{ Math.round(j.progress * 100) }}%</span>
        </div>
      </div>
      <div v-else-if="jobs.jobs.length > 0" class="jobs">
        <p class="hint">Recent jobs:</p>
        <div v-for="j in jobs.jobs.slice(0, 5)" :key="j.id" class="job done">
          <span class="job-label">{{ j.kind === "scan" ? "Library scan" : j.label }}</span>
          <span class="badge" :class="j.status === 'done' ? 'ok' : 'bad'">{{ j.status }}</span>
        </div>
        <button class="icon-btn" @click="jobs.refresh()">⟳ Refresh</button>
      </div>
    </section>

    <section>
      <h3>Audio output</h3>
      <p class="hint">
        PCM plays through the default device in shared mode. There is no
        output-device selector in v1 — the engine always uses the system
        default (rebuilding the audio sink around a chosen device would
        require tearing down the playback pipeline; documented limitation).
        On macOS, DSD tracks can use exclusive hog-mode DoP output instead —
        bit-perfect, bypassing the EQ, loudness, and volume below.
      </p>
      <ul v-if="dsp.devices.length" class="dev-list">
        <li v-for="d in dsp.devices" :key="d.name">
          {{ d.name }}<span v-if="d.is_default" class="badge">default</span>
        </li>
      </ul>
      <p v-else class="hint">No output devices reported.</p>
      <p class="hint">
        <template v-if="dsp.dop?.exclusive_available">
          Exclusive DoP is available on this Mac.
          <template v-if="dopRates">Device accepts DoP at {{ dopRates }}.</template>
          <template v-else>
            The current output device reports no DoP-capable rate — DSD falls back
            to the FLAC transcode and the reason is logged.
          </template>
        </template>
        <template v-else>Exclusive DoP output is macOS-only.</template>
      </p>
    </section>

    <section>
      <h3>Parametric EQ</h3>
      <p class="hint">
        Up to 8 bands, applied to PCM only (DoP bypasses EQ). Changes apply live
        and are saved through the Rust core.
      </p>
      <label class="check">
        <input
          type="checkbox"
          :checked="dsp.eqEnabled"
          @change="dsp.saveEqEnabled(($event.target as HTMLInputElement).checked)"
        />
        EQ enabled
      </label>
      <div v-if="dsp.rowError" class="error-banner">{{ dsp.rowError }}</div>
      <div class="bands">
        <div v-for="(b, i) in dsp.rows" :key="i" class="band" :class="{ off: !b.enabled }">
          <input
            type="checkbox"
            :checked="b.enabled"
            title="Enable this band"
            @change="dsp.toggleRow(i)"
          />
          <select :value="b.band_type" title="Band type" @change="onBandType(i, $event)">
            <option v-for="t in EQ_BAND_TYPES" :key="t" :value="t">{{ bandTypeLabel(t) }}</option>
          </select>
          <label title="Frequency (Hz)"
            >Hz <input type="number" :value="b.freq" min="10" max="24000" step="1" @change="onBandNum(i, 'freq', $event)"
          /></label>
          <label title="Gain (dB)"
            >dB
            <input type="number" :value="b.gain_db" min="-24" max="24" step="0.5" @change="onBandNum(i, 'gain_db', $event)"
          /></label>
          <label title="Q (shelf slope for shelves)"
            >Q <input type="number" :value="b.q" min="0.1" max="18" step="0.1" @change="onBandNum(i, 'q', $event)"
          /></label>
          <button class="icon-btn" title="Remove band" @click="dsp.removeBand(i)">✕</button>
        </div>
      </div>
      <button class="icon-btn" :disabled="!dsp.canAddBand" @click="dsp.addBand()">
        ＋ Add band
      </button>
    </section>

    <section>
      <h3>Loudness normalization</h3>
      <p class="hint">
        EBU R128-style loudness matching for PCM only (DoP bypasses it). The first
        play of each track does a fast pre-scan — one extra stream, roughly double
        the bandwidth — then the measured gain is cached.
      </p>
      <label class="check">
        <input
          type="checkbox"
          :checked="dsp.loudnessEnabled"
          @change="dsp.saveLoudnessEnabled(($event.target as HTMLInputElement).checked)"
        />
        Loudness normalization enabled
      </label>
      <div class="row">
        <label class="lufs">
          Target
          <input
            v-model="loudnessInput"
            type="number"
            min="-40"
            max="-1"
            step="0.5"
            @change="onLoudnessTarget"
          />
          LUFS
        </label>
      </div>
      <div v-if="loudnessError" class="error-banner">{{ loudnessError }}</div>
    </section>

    <section>
      <h3>Keyboard shortcuts</h3>
      <p class="hint">Available everywhere except while typing in a text field.</p>
      <ul class="keys">
        <li v-for="[key, desc] in KEYBOARD_MAP" :key="key">
          <kbd>{{ key }}</kbd><span>{{ desc }}</span>
        </li>
      </ul>
    </section>
  </div>
</template>

<style scoped>
.settings {
  max-width: 640px;
}

section {
  margin-bottom: 28px;
}

h3 {
  margin: 0 0 4px;
  font-size: 14px;
}

.hint {
  color: var(--text-dim);
  font-size: 12px;
  margin: 0 0 10px;
}

.hint code {
  font-family: ui-monospace, monospace;
  font-size: 11px;
}

.row {
  display: flex;
  gap: 8px;
}

.row input {
  flex: 1;
}

.status {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-dim);
  font-size: 12px;
  margin: 8px 0 0;
}

.status.ok {
  color: #30d158;
}

.status.bad {
  color: var(--danger);
}

.dev-list {
  list-style: none;
  padding: 0;
  margin: 0 0 8px;
  font-size: 13px;
}

.dev-list li {
  padding: 4px 0;
  border-bottom: 1px solid var(--border);
}

.dev-list .badge {
  margin-left: 8px;
}

.check {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  margin-bottom: 10px;
}

.bands {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-bottom: 10px;
}

.band {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 8px;
  font-size: 12px;
}

.band.off {
  opacity: 0.45;
}

.band select {
  min-width: 110px;
}

.band label {
  display: flex;
  align-items: center;
  gap: 4px;
  color: var(--text-dim);
}

.band input[type="number"] {
  width: 76px;
}

.lufs {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
}

.lufs input {
  width: 70px;
}

.keys {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.keys li {
  display: flex;
  align-items: center;
  gap: 12px;
  font-size: 13px;
}

.keys kbd {
  font-family: ui-monospace, monospace;
  font-size: 11px;
  background: var(--bg-active);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 2px 8px;
  min-width: 64px;
  text-align: center;
}

.jobs {
  margin-top: 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.job {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 13px;
}

.job.done {
  color: var(--text-dim);
}

.job-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.job-bar {
  flex: 2;
  height: 6px;
  background: var(--bg-active);
  border-radius: 3px;
  overflow: hidden;
  max-width: 200px;
}

.job-fill {
  height: 100%;
  background: #0a84ff;
  transition: width 0.4s linear;
}

.job-pct {
  font-variant-numeric: tabular-nums;
  color: var(--text-dim);
  font-size: 12px;
  min-width: 36px;
  text-align: right;
}

.badge.ok {
  background: rgba(48, 209, 88, 0.16);
  color: #30d158;
}

.badge.bad {
  background: rgba(255, 69, 58, 0.14);
  color: #ff9d97;
}
</style>
