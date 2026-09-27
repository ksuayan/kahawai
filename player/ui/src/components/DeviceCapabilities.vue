<script setup lang="ts">
import { computed } from "vue";
import type { DeviceCapabilities } from "../types";
import UiBadge from "../ui/UiBadge.vue";

/**
 * What the selected output device can carry, at a glance: every standard
 * rate, bit depth and DSD rate is shown, with the ones the device can't do
 * dimmed. Read-only; the device's own report, not a guess.
 */
const props = defineProps<{
  caps: DeviceCapabilities;
  /** Built in, or confirmed by the user, as decoding DoP. */
  knownDsd?: boolean;
}>();

const RATES = [44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000];
const DSD = [
  { name: "DSD64", dop: 176400 },
  { name: "DSD128", dop: 352800 },
  { name: "DSD256", dop: 705600 },
  { name: "DSD512", dop: 1411200 },
];

const kHz = (r: number): string => `${Math.round(r / 100) / 10}`;

const transportLabel = computed(
  () =>
    ({
      usb: "USB",
      thunderbolt: "Thunderbolt",
      firewire: "FireWire",
      "built-in": "Built-in",
      bluetooth: "Bluetooth",
      hdmi: "HDMI / DisplayPort",
      airplay: "AirPlay",
      virtual: "Virtual",
    })[props.caps.transport] ?? "Other",
);

const rates = computed(() => RATES.map((r) => ({ r, ok: props.caps.sample_rates.includes(r) })));

// A 32-bit integer slot usually carries a 24-bit DAC word, so 24 counts too.
const depths = computed(() => [
  { d: 16, ok: props.caps.bit_depths.includes(16), note: "" },
  {
    d: 24,
    ok: props.caps.bit_depths.includes(24) || props.caps.bit_depths.includes(32),
    note: props.caps.bit_depths.includes(24) ? "" : " (in a 32-bit slot)",
  },
  { d: 32, ok: props.caps.bit_depths.includes(32), note: "" },
]);

const dsd = computed(() => DSD.map((x) => ({ ...x, ok: props.caps.dop_rates.includes(x.dop) })));

const chip = (ok: boolean): string =>
  ok ? "border-line bg-surface text-fg" : "border-line/50 text-dim opacity-40 line-through";
</script>

<template>
  <div class="my-2 rounded-lg border border-line bg-raised p-3 text-sm" data-testid="device-capabilities">
    <div class="mb-2 flex flex-wrap items-center gap-1.5">
      <span class="font-semibold" data-testid="cap-name">{{ caps.name.trim() || "Output device" }}</span>
      <UiBadge data-testid="cap-transport">{{ transportLabel }}</UiBadge>
      <UiBadge v-if="caps.external_dac" variant="ok" data-testid="cap-external">External DAC</UiBadge>
      <UiBadge v-else data-testid="cap-external">Not an external DAC</UiBadge>
      <UiBadge v-if="caps.exclusive_available" :variant="caps.external_dac ? 'ok' : 'default'" data-testid="cap-exclusive">
        Exclusive mode
      </UiBadge>
    </div>

    <dl class="m-0 grid grid-cols-[auto_1fr] items-baseline gap-x-3 gap-y-1.5">
      <dt class="text-dim">Sample rates</dt>
      <dd class="m-0 flex flex-wrap gap-1" data-testid="cap-rates">
        <span
          v-for="x in rates"
          :key="x.r"
          class="rounded-sm border px-1.5 py-0.5 text-[11px] tabular-nums"
          :class="chip(x.ok)"
          :data-ok="x.ok"
          :title="x.ok ? `${kHz(x.r)} kHz supported` : `${kHz(x.r)} kHz not offered`"
        >{{ kHz(x.r) }}</span>
        <span class="self-center text-[11px] text-dim">kHz</span>
      </dd>

      <dt class="text-dim">Bit depth</dt>
      <dd class="m-0 flex flex-wrap gap-1" data-testid="cap-depths">
        <span
          v-for="x in depths"
          :key="x.d"
          class="rounded-sm border px-1.5 py-0.5 text-[11px]"
          :class="chip(x.ok)"
          :data-ok="x.ok"
          :title="x.ok ? `${x.d}-bit${x.note}` : `${x.d}-bit not offered`"
        >{{ x.d }}-bit{{ x.note }}</span>
      </dd>

      <dt class="text-dim">DSD (DoP)</dt>
      <dd class="m-0 flex flex-wrap items-center gap-1" data-testid="dsd-rate-probe">
        <span
          v-for="x in dsd"
          :key="x.name"
          class="rounded-sm border px-1.5 py-0.5 text-[11px]"
          :class="chip(x.ok)"
          :data-ok="x.ok"
          :title="x.ok ? `${x.name} can be carried over DoP at ${kHz(x.dop)} kHz` : `${x.name} needs ${kHz(x.dop)} kHz, which this device doesn't offer`"
        >{{ x.name }} {{ x.ok ? "✓" : "✗" }}</span>
        <span class="text-[11px]" :class="knownDsd ? 'text-ok' : 'text-dim'" data-testid="cap-dsd-known">
          {{ knownDsd ? "Known to decode DoP" : "Not confirmed to decode DoP" }}
        </span>
      </dd>
    </dl>
  </div>
</template>
