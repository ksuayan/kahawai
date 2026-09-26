<script setup lang="ts">
import { Plus, Trash2, TriangleAlert } from "lucide-vue-next";
import { computed, ref, watch } from "vue";
import { bandHasGain, constrainBand, DEFAULT_RATE_HZ, EQ_LIMITS, maxFreqFor, qRange, totalResponseDb } from "../eqResponse";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import { EQ_BAND_TYPES, MAX_EQ_BANDS, type EqBandType } from "../types";
import PromptDialog from "../ui/PromptDialog.vue";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/**
 * Interactive EQ: the combined frequency response with a draggable node per
 * band (drag = frequency + gain, arrows on a focused node, double-click the
 * graph to add a band), preset picker and "save as preset". Applies live.
 * Exclusive outputs (DoP, bit-perfect) bypass the EQ, so the editor is dimmed.
 */
const open = defineModel<boolean>("open", { required: true });
const dsp = useDspStore();
const player = usePlayerStore();

const selected = ref<number | null>(null);
// Edits apply live so the change is audible; Cancel (or closing the window)
// puts back what was there when the dialog opened, OK keeps it.
let snap: ReturnType<typeof dsp.snapshotEq> | null = null;
watch(
  open,
  (o) => {
    if (o) {
      snap = dsp.snapshotEq();
      selected.value = null;
    }
  },
  { immediate: true },
);
function ok(): void {
  snap = null;
  open.value = false;
}
function cancel(): void {
  if (snap) void dsp.restoreEq(snap);
  snap = null;
  open.value = false;
}
/** Escape / overlay click / close all count as Cancel. */
function onOpenChange(v: boolean): void {
  if (v) open.value = true;
  else cancel();
}

const unsupported = computed(() => player.isExclusive);
// The EQ is designed at the rate the audio reaches the output; draw and limit for that.
const rate = computed(() => player.outputRateHz ?? DEFAULT_RATE_HZ);
const maxFreq = computed(() => maxFreqFor(rate.value));
const rateLabel = computed(() => `${Math.round(rate.value / 100) / 10} kHz`);
const UNSUPPORTED_TEXT = "EQ is not supported for this stream type.";

// --- graph geometry ---------------------------------------------------------
const W = 760, H = 300, PAD_L = 40, PAD_R = 12, PAD_T = 12, PAD_B = 24;
const F_MIN = 20, F_MAX = 20000, DB = 18;
const plotW = W - PAD_L - PAD_R, plotH = H - PAD_T - PAD_B;
const lg = (f: number) => Math.log10(f);
const xOf = (f: number) => PAD_L + ((lg(f) - lg(F_MIN)) / (lg(F_MAX) - lg(F_MIN))) * plotW;
const yOf = (db: number) => PAD_T + ((DB - db) / (2 * DB)) * plotH;
const fOf = (x: number) => 10 ** (lg(F_MIN) + ((x - PAD_L) / plotW) * (lg(F_MAX) - lg(F_MIN)));
const dbOf = (y: number) => DB - ((y - PAD_T) / plotH) * 2 * DB;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

const FREQ_GRID = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
const DB_GRID = [-12, -6, 0, 6, 12];
const fLabel = (f: number) => (f >= 1000 ? `${f / 1000}k` : String(f));

/** Sampled response (dB) across the axis, unclamped. */
const samples = computed(() =>
  Array.from({ length: 161 }, (_, i) => {
    const f = F_MIN * (F_MAX / F_MIN) ** (i / 160);
    return { f, db: totalResponseDb(dsp.activeBands, f, rate.value) };
  }),
);
const curve = computed(() =>
  samples.value
    .map((p, i) => `${i === 0 ? "M" : "L"}${xOf(p.f).toFixed(1)},${yOf(clamp(p.db, -DB, DB)).toFixed(1)}`)
    .join(" "),
);
/** Highest boost of the combined curve; above 0 dB loud material can clip. */
const peakDb = computed(() => Math.max(0, ...samples.value.map((p) => p.db)));
const clipRisk = computed(() => dsp.eqEnabled && peakDb.value > 0.5);
const fill = computed(() => `${curve.value} L${xOf(F_MAX)},${yOf(0)} L${xOf(F_MIN)},${yOf(0)} Z`);

const hasGain = bandHasGain;

const sel = computed(() => (selected.value === null ? null : (dsp.rows[selected.value] ?? null)));

// --- interaction ------------------------------------------------------------
const svg = ref<SVGSVGElement | null>(null);
let dragging: number | null = null;

function pointFromEvent(e: PointerEvent | MouseEvent): { f: number; db: number } {
  const r = svg.value!.getBoundingClientRect();
  const x = ((e.clientX - r.left) / r.width) * W;
  const y = ((e.clientY - r.top) / r.height) * H;
  return { f: clamp(fOf(x), F_MIN, F_MAX), db: clamp(dbOf(y), -DB, DB) };
}

function move(i: number, f: number, db: number): void {
  const r = dsp.rows[i];
  if (!r) return;
  const c = constrainBand({ ...r, freq: f, gain_db: Math.round(db * 2) / 2 }, rate.value);
  dsp.updateRow(i, hasGain(r.band_type) ? { freq: Math.round(c.freq), gain_db: c.gain_db } : { freq: Math.round(c.freq) });
}

function onDown(i: number, e: PointerEvent): void {
  if (unsupported.value) return;
  selected.value = i;
  dragging = i;
  (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
  e.preventDefault();
}
function onMove(e: PointerEvent): void {
  if (dragging === null) return;
  const p = pointFromEvent(e);
  move(dragging, p.f, p.db);
}
function onUp(): void {
  dragging = null;
}
function onKey(i: number, e: KeyboardEvent): void {
  const r = dsp.rows[i];
  if (!r || unsupported.value) return;
  const step = e.shiftKey ? 3 : 1;
  const k = e.key;
  if (k === "ArrowLeft") move(i, r.freq / 1.05 ** step, r.gain_db);
  else if (k === "ArrowRight") move(i, r.freq * 1.05 ** step, r.gain_db);
  else if (k === "ArrowUp") move(i, r.freq, r.gain_db + 0.5 * step);
  else if (k === "ArrowDown") move(i, r.freq, r.gain_db - 0.5 * step);
  else if (k === "Delete" || k === "Backspace") removeSelected();
  else return;
  e.preventDefault();
}
function onAdd(e: MouseEvent): void {
  if (unsupported.value || !dsp.canAddBand) return;
  const p = pointFromEvent(e);
  dsp.addBand();
  const i = dsp.rows.length - 1;
  selected.value = i;
  move(i, p.f, p.db);
}
function removeSelected(): void {
  if (selected.value === null) return;
  dsp.removeBand(selected.value);
  selected.value = null;
}

// --- band fields ------------------------------------------------------------
const typeLabel: Record<EqBandType, string> = {
  peaking: "Peaking",
  low_shelf: "Low shelf",
  high_shelf: "High shelf",
  low_pass: "Low-pass",
  high_pass: "High-pass",
};
const typeOptions: UiSelectOption[] = EQ_BAND_TYPES.map((t) => ({ value: t, label: typeLabel[t] }));
function onNum(field: "freq" | "gain_db" | "q", e: Event): void {
  const i = selected.value;
  const r = i === null ? null : dsp.rows[i];
  if (i === null || !r) return;
  const el = e.target as HTMLInputElement;
  const c = constrainBand({ ...r, [field]: Number(el.value) }, rate.value);
  el.value = String(c[field]); // show what was actually applied
  dsp.updateRow(i, { [field]: c[field] });
}
function onType(v: string | null): void {
  const i = selected.value;
  const r = i === null ? null : dsp.rows[i];
  if (i === null || !r || !v) return;
  const c = constrainBand({ ...r, band_type: v as EqBandType }, rate.value);
  dsp.updateRow(i, { band_type: c.band_type, gain_db: c.gain_db, q: c.q });
}

// --- presets ----------------------------------------------------------------
const CUSTOM = "custom";
const naming = ref(false);
const presetOptions = computed<UiSelectOption[]>(() => [
  ...(dsp.activePreset ? [] : [{ value: CUSTOM, label: "Custom", disabled: true }]),
  ...dsp.presets.map((p) => ({ value: p.id, label: p.builtin ? p.name : `${p.name} (mine)` })),
]);
const presetValue = computed(() => dsp.activePreset?.id ?? CUSTOM);
const userPresetActive = computed(() => !!dsp.activePreset && !dsp.activePreset.builtin);
function choose(v: string | null): void {
  if (v && v !== CUSTOM) {
    selected.value = null;
    void dsp.applyPreset(v);
  }
}
</script>

<template>
  <UiDialog :open="open" wide @update:open="onOpenChange" title="Equalizer" description="Drag a point to shape the sound. Double-click the graph to add a band.">
    <p v-if="unsupported" class="mb-3 rounded-md border border-line bg-active px-3 py-2 text-sm text-dim" role="status" data-testid="eq-unsupported">
      {{ UNSUPPORTED_TEXT }}
    </p>
    <div
      :class="unsupported ? 'pointer-events-none opacity-40 grayscale' : ''"
      :aria-disabled="unsupported || undefined"
      :inert="unsupported || undefined"
      data-testid="eq-editor"
    >
      <div class="mb-3 flex flex-wrap items-center gap-3">
        <UiSwitch :model-value="dsp.eqEnabled" label="EQ enabled" @update:model-value="(v) => dsp.saveEqEnabled(v)" />
        <UiSelect
          aria-label="EQ preset"
          trigger-class="w-[200px]"
          :model-value="presetValue"
          :options="presetOptions"
          @update:model-value="choose"
        />
        <UiButton v-if="userPresetActive" variant="icon-danger" title="Delete this preset" aria-label="Delete preset" @click="dsp.deleteUserPreset(dsp.activePreset!.id)">
          <Trash2 />
        </UiButton>
        <UiButton :disabled="dsp.activeBands.length === 0" data-testid="save-preset" @click="naming = true">Save as preset…</UiButton>
      </div>

      <svg
        ref="svg"
        :viewBox="`0 0 ${W} ${H}`"
        class="w-full touch-none select-none rounded-lg border border-line bg-canvas"
        :class="dsp.eqEnabled ? '' : 'opacity-50'"
        role="group"
        aria-label="EQ frequency response"
        data-testid="eq-graph"
        @pointermove="onMove"
        @pointerup="onUp"
        @pointercancel="onUp"
        @dblclick="onAdd"
      >
        <g class="text-faint" font-size="10" fill="currentColor">
          <template v-for="f in FREQ_GRID" :key="f">
            <line :x1="xOf(f)" :x2="xOf(f)" :y1="PAD_T" :y2="PAD_T + plotH" stroke="currentColor" stroke-opacity="0.18" />
            <text :x="xOf(f)" :y="H - 8" text-anchor="middle">{{ fLabel(f) }}</text>
          </template>
          <template v-for="d in DB_GRID" :key="d">
            <line :x1="PAD_L" :x2="PAD_L + plotW" :y1="yOf(d)" :y2="yOf(d)" stroke="currentColor" :stroke-opacity="d === 0 ? 0.5 : 0.18" />
            <text :x="PAD_L - 6" :y="yOf(d) + 3" text-anchor="end">{{ d > 0 ? "+" : "" }}{{ d }}</text>
          </template>
        </g>
        <path :d="fill" class="fill-accent" fill-opacity="0.15" data-testid="eq-fill" />
        <path :d="curve" fill="none" class="stroke-accent" stroke-width="2" data-testid="eq-curve" />
        <g v-for="(r, i) in dsp.rows" :key="i">
          <circle
            :cx="xOf(Math.min(r.freq, maxFreq))"
            :cy="yOf(hasGain(r.band_type) ? clamp(r.gain_db, -DB, DB) : 0)"
            :r="selected === i ? 9 : 7"
            :class="[r.enabled ? 'fill-accent' : 'fill-faint', selected === i ? 'stroke-fg' : 'stroke-canvas']"
            stroke-width="2"
            class="cursor-grab outline-none focus-visible:stroke-fg active:cursor-grabbing"
            tabindex="0"
            role="slider"
            :aria-label="`Band ${i + 1}: ${typeLabel[r.band_type]} ${r.freq} Hz ${r.gain_db} dB`"
            :aria-valuenow="r.gain_db"
            aria-valuemin="-24"
            aria-valuemax="24"
            data-testid="eq-node"
            @pointerdown="onDown(i, $event)"
            @keydown="onKey(i, $event)"
            @focus="selected = i"
            @dblclick.stop
          />
        </g>
      </svg>

      <p class="m-0 mt-1.5 text-[11px] text-faint" data-testid="eq-rate">
        Curve for {{ rateLabel }} output{{ player.outputRateHz ? "" : " (nothing playing)" }}. Bands are limited to {{ maxFreq }} Hz and ±{{ EQ_LIMITS.gainMax }} dB.
      </p>
      <p v-if="clipRisk" class="m-0 mt-2 flex items-start gap-1.5 text-xs text-warn-fg" role="status" data-testid="eq-headroom">
        <TriangleAlert class="mt-px size-3.5 shrink-0" />
        Peak boost of +{{ peakDb.toFixed(1) }} dB can clip loud tracks. Lower the boost, or cut the loud bands instead.
      </p>
      <p v-if="sel && sel.freq > maxFreq" class="m-0 mt-2 text-xs text-warn-fg" role="status" data-testid="eq-band-capped">
        Band {{ selected! + 1 }} is above the usable range at {{ rateLabel }} and is applied at {{ maxFreq }} Hz.
      </p>

      <div class="mt-3 flex min-h-[44px] flex-wrap items-center gap-3 text-xs text-dim" data-testid="eq-band-panel">
        <template v-if="sel && selected !== null">
          <span class="font-semibold text-fg">Band {{ selected + 1 }}</span>
          <UiSelect aria-label="Band type" trigger-class="w-[130px]" :model-value="sel.band_type" :options="typeOptions" @update:model-value="onType" />
          <label class="flex items-center gap-1">Hz <UiInput class="w-[80px]" type="number" :model-value="String(sel.freq)" min="20" :max="maxFreq" @change="onNum('freq', $event)" /></label>
          <label v-if="hasGain(sel.band_type)" class="flex items-center gap-1">dB <UiInput class="w-[70px]" type="number" :model-value="String(sel.gain_db)" min="-18" max="18" step="0.5" @change="onNum('gain_db', $event)" /></label>
          <label class="flex items-center gap-1">{{ sel.band_type.endsWith("shelf") ? "Slope" : "Q" }} <UiInput class="w-[70px]" type="number" :model-value="String(sel.q)" :min="qRange(sel.band_type).min" :max="qRange(sel.band_type).max" step="0.1" @change="onNum('q', $event)" /></label>
          <UiSwitch :model-value="sel.enabled" label="On" @update:model-value="dsp.toggleRow(selected!)" />
          <UiButton variant="icon-danger" title="Remove band" aria-label="Remove band" @click="removeSelected"><Trash2 /></UiButton>
        </template>
        <template v-else>
          <span>Select a point to edit it.</span>
          <UiButton :disabled="!dsp.canAddBand" :title="dsp.canAddBand ? undefined : `At most ${MAX_EQ_BANDS} bands`" data-testid="add-band" @click="dsp.addBand(); selected = dsp.rows.length - 1"><Plus /> Add band</UiButton>
        </template>
        <span v-if="dsp.rowError" class="text-danger" role="alert">{{ dsp.rowError }}</span>
      </div>
    </div>
    <template #footer>
      <UiButton data-testid="eq-cancel" @click="cancel">Cancel</UiButton>
      <UiButton variant="primary" data-testid="eq-ok" @click="ok">OK</UiButton>
    </template>
    <PromptDialog v-model:open="naming" title="Save EQ preset" label="Preset name" placeholder="My tuning" confirm-label="Save" :maxlength="40" @submit="(n) => dsp.saveUserPreset(n)" />
  </UiDialog>
</template>
