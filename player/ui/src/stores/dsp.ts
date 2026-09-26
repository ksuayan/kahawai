import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  dopStatus,
  getDspSettings,
  getOutputDevice,
  getOutputDevices,
  setOutputDevice,
  setEqBands,
  setEqEnabled,
  setLoudnessEnabled,
  setLoudnessTarget,
} from "../tauri";
import { BUILTIN_PRESETS, sameBands, type EqPreset } from "../eqPresets";
import {
  DEFAULT_DSP_SETTINGS,
  MAX_EQ_BANDS,
  type DopStatus,
  type EqBand,
  type EqBandRow,
  type OutputDevice,
} from "../types";

const ROWS_KEY = "kahawai-player.eq-rows";
const PRESETS_KEY = "kahawai-player.eq-user-presets";

function loadUserPresets(): EqPreset[] {
  try {
    const raw = localStorage.getItem(PRESETS_KEY);
    const list = raw ? (JSON.parse(raw) as { name: string; bands: EqBand[] }[]) : [];
    return list.map((p) => ({ id: `user:${p.name}`, name: p.name, bands: p.bands, builtin: false }));
  } catch {
    return [];
  }
}

/** Client-side mirror of the Rust `validate_bands` limits. */
export function validateRow(r: EqBandRow): string | null {
  if (!isFinite(r.freq) || r.freq < 10 || r.freq > 24000) return "Frequency must be 10–24000 Hz.";
  if (!isFinite(r.gain_db) || r.gain_db < -24 || r.gain_db > 24)
    return "Gain must be −24…+24 dB.";
  if (!isFinite(r.q) || r.q < 0.1 || r.q > 18) return "Q must be 0.1–18.";
  return null;
}

/**
 * DSP settings: parametric EQ + loudness normalization. The Rust engine is
 * the source of truth for what is *applied* (persisted in
 * engine-settings.json); this store keeps the UI row model, including
 * per-row enable switches, in localStorage. Disabled rows are excluded
 * from the band list pushed to the core. Both EQ and loudness are
 * PCM-only — the exclusive DoP path bypasses them bit-perfectly.
 */
export const useDspStore = defineStore("dsp", () => {
  const rows = ref<EqBandRow[]>([]);
  const eqEnabled = ref(true);
  const loudnessEnabled = ref(false);
  const loudnessTarget = ref(-14);
  const devices = ref<OutputDevice[]>([]);
  /** Chosen output device (exact name); null = follow the system default. */
  const outputDevice = ref<string | null>(null);
  const dop = ref<DopStatus | null>(null);
  const loaded = ref(false);
  const rowError = ref<string | null>(null);

  const activeBands = computed<EqBand[]>(() =>
    rows.value.filter((r) => r.enabled).map(({ enabled: _e, ...b }) => b),
  );
  const userPresets = ref<EqPreset[]>(loadUserPresets());
  const presets = computed<EqPreset[]>(() => [...BUILTIN_PRESETS, ...userPresets.value]);
  /** The preset the enabled rows currently equal; null = a custom tuning. */
  const activePreset = computed<EqPreset | null>(
    () => presets.value.find((p) => sameBands(p.bands, activeBands.value)) ?? null,
  );
  const canAddBand = computed(() => rows.value.length < MAX_EQ_BANDS);

  function persistRows(): void {
    try {
      localStorage.setItem(ROWS_KEY, JSON.stringify(rows.value));
    } catch {
      /* storage unavailable — rows stay session-local */
    }
  }

  async function pushBands(): Promise<void> {
    await setEqBands(activeBands.value);
    persistRows();
  }

  /** On boot: engine settings first, then overlay the UI row model. */
  async function init(): Promise<void> {
    const [s, devs, d, chosen] = await Promise.all([
      getDspSettings(),
      getOutputDevices(),
      dopStatus(),
      getOutputDevice(),
    ]);
    const dsp = s ?? DEFAULT_DSP_SETTINGS;
    eqEnabled.value = dsp.eq_enabled;
    loudnessEnabled.value = dsp.loudness_enabled;
    loudnessTarget.value = dsp.loudness_target;
    devices.value = devs ?? [];
    dop.value = d ?? null;
    outputDevice.value = chosen;

    let restored: EqBandRow[] | null = null;
    try {
      const raw = localStorage.getItem(ROWS_KEY);
      if (raw) restored = JSON.parse(raw) as EqBandRow[];
    } catch {
      restored = null;
    }
    if (restored && restored.length <= MAX_EQ_BANDS) {
      rows.value = restored;
      // Reconcile: the row model (with disabled rows) wins, push the
      // enabled subset so the engine matches what the UI shows.
      await pushBands();
    } else {
      rows.value = dsp.eq_bands.map((b) => ({ ...b, enabled: true }));
    }
    loaded.value = true;
  }

  /** Re-scan devices (hot-plugged DACs) and the DoP rates of the chosen one. */
  async function refreshDevices(): Promise<void> {
    const [devs, d] = await Promise.all([getOutputDevices(), dopStatus()]);
    devices.value = devs ?? [];
    dop.value = d ?? null;
  }

  /** Route playback to a device (null = system default). A playing track
   *  moves over at its current position. Names are passed through exactly
   *  (some contain trailing spaces or non-ASCII). */
  async function chooseOutputDevice(name: string | null): Promise<void> {
    outputDevice.value = name;
    await setOutputDevice(name);
    dop.value = (await dopStatus()) ?? dop.value;
  }

  /** The chosen device is not in the current device list (unplugged). */
  const outputDeviceMissing = computed(
    () => outputDevice.value !== null && !devices.value.some((d) => d.name === outputDevice.value),
  );

  function persistPresets(): void {
    try {
      localStorage.setItem(
        PRESETS_KEY,
        JSON.stringify(userPresets.value.map((p) => ({ name: p.name, bands: p.bands }))),
      );
    } catch {
      /* storage unavailable — presets stay session-local */
    }
  }

  /** Replace the EQ rows with a preset's bands (turning EQ on for anything but Flat). */
  async function applyPreset(id: string): Promise<void> {
    const p = presets.value.find((x) => x.id === id);
    if (!p) return;
    rows.value = p.bands.map((b) => ({ ...b, enabled: true }));
    rowError.value = null;
    await pushBands();
    if (p.bands.length > 0 && !eqEnabled.value) await saveEqEnabled(true);
  }

  /** Save the current enabled bands as a user preset (same name overwrites). */
  function saveUserPreset(name: string): void {
    const clean = name.trim();
    if (!clean) return;
    const preset: EqPreset = {
      id: `user:${clean}`,
      name: clean,
      bands: activeBands.value.map((b) => ({ ...b })),
      builtin: false,
    };
    userPresets.value = [...userPresets.value.filter((p) => p.id !== preset.id), preset];
    persistPresets();
  }

  function deleteUserPreset(id: string): void {
    userPresets.value = userPresets.value.filter((p) => p.id !== id);
    persistPresets();
  }

  /** Capture the EQ state so an editor session can be cancelled. */
  function snapshotEq(): { rows: EqBandRow[]; enabled: boolean } {
    return { rows: rows.value.map((r) => ({ ...r })), enabled: eqEnabled.value };
  }

  /** Put back a state taken with `snapshotEq` (edits apply live, so Cancel = undo them). */
  async function restoreEq(snap: { rows: EqBandRow[]; enabled: boolean }): Promise<void> {
    rows.value = snap.rows.map((r) => ({ ...r }));
    rowError.value = null;
    await pushBands();
    if (eqEnabled.value !== snap.enabled) await saveEqEnabled(snap.enabled);
  }

  function addBand(): void {
    if (!canAddBand.value) return;
    rows.value.push({ band_type: "peaking", freq: 1000, gain_db: 0, q: 1.0, enabled: true });
    rowError.value = null;
    void pushBands();
  }

  function removeBand(i: number): void {
    rows.value.splice(i, 1);
    rowError.value = null;
    void pushBands();
  }

  function toggleRow(i: number): void {
    const r = rows.value[i];
    if (!r) return;
    r.enabled = !r.enabled;
    void pushBands();
  }

  /** Update one row; validates before pushing. Returns false when rejected. */
  function updateRow(i: number, patch: Partial<EqBand>): boolean {
    const r = rows.value[i];
    if (!r) return false;
    const next = { ...r, ...patch };
    const err = validateRow(next);
    if (err) {
      rowError.value = err;
      return false;
    }
    rowError.value = null;
    Object.assign(r, patch);
    void pushBands();
    return true;
  }

  async function saveEqEnabled(v: boolean): Promise<void> {
    eqEnabled.value = v;
    await setEqEnabled(v);
  }

  async function saveLoudnessEnabled(v: boolean): Promise<void> {
    loudnessEnabled.value = v;
    await setLoudnessEnabled(v);
  }

  /** Returns false when the target is out of the −40…−1 LUFS range. */
  async function saveLoudnessTarget(v: number): Promise<boolean> {
    if (!isFinite(v) || v < -40 || v > -1) return false;
    loudnessTarget.value = v;
    await setLoudnessTarget(v);
    return true;
  }

  return {
    rows,
    eqEnabled,
    loudnessEnabled,
    loudnessTarget,
    devices,
    outputDevice,
    outputDeviceMissing,
    dop,
    loaded,
    rowError,
    activeBands,
    canAddBand,
    presets,
    userPresets,
    activePreset,
    applyPreset,
    snapshotEq,
    restoreEq,
    saveUserPreset,
    deleteUserPreset,
    init,
    refreshDevices,
    chooseOutputDevice,
    addBand,
    removeBand,
    toggleRow,
    updateRow,
    saveEqEnabled,
    saveLoudnessEnabled,
    saveLoudnessTarget,
  };
});
