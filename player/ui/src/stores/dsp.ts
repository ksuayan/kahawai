import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  dopStatus,
  getDspSettings,
  getOutputDevices,
  setEqBands,
  setEqEnabled,
  setLoudnessEnabled,
  setLoudnessTarget,
} from "../tauri";
import {
  DEFAULT_DSP_SETTINGS,
  MAX_EQ_BANDS,
  type DopStatus,
  type EqBand,
  type EqBandRow,
  type OutputDevice,
} from "../types";

const ROWS_KEY = "kahawai-player.eq-rows";

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
  const dop = ref<DopStatus | null>(null);
  const loaded = ref(false);
  const rowError = ref<string | null>(null);

  const activeBands = computed<EqBand[]>(() =>
    rows.value.filter((r) => r.enabled).map(({ enabled: _e, ...b }) => b),
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
    const [s, devs, d] = await Promise.all([getDspSettings(), getOutputDevices(), dopStatus()]);
    const dsp = s ?? DEFAULT_DSP_SETTINGS;
    eqEnabled.value = dsp.eq_enabled;
    loudnessEnabled.value = dsp.loudness_enabled;
    loudnessTarget.value = dsp.loudness_target;
    devices.value = devs ?? [];
    dop.value = d ?? null;

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
    dop,
    loaded,
    rowError,
    activeBands,
    canAddBand,
    init,
    addBand,
    removeBand,
    toggleRow,
    updateRow,
    saveEqEnabled,
    saveLoudnessEnabled,
    saveLoudnessTarget,
  };
});
