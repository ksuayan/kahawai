<script setup lang="ts">
import { SlidersVertical, Trash2 } from "lucide-vue-next";
import { PopoverContent, PopoverPortal, PopoverRoot, PopoverTrigger } from "reka-ui";
import { computed, ref } from "vue";
import { useDspStore } from "../stores/dsp";
import { usePlayerStore } from "../stores/player";
import EqDialog from "./EqDialog.vue";
import PromptDialog from "../ui/PromptDialog.vue";
import UiButton from "../ui/UiButton.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/**
 * Quick EQ: a preset picker (built-in tunings plus the user's own) next to
 * shuffle/repeat. The full parametric editor stays in Settings; a tuning
 * that matches no preset shows as "Custom".
 */
const props = withDefaults(defineProps<{ size?: "md" | "lg"; disabled?: boolean }>(), { size: "md", disabled: false });
const dsp = useDspStore();
const player = usePlayerStore();
const graph = ref(false);
const unsupported = computed(() => player.isExclusive);

const CUSTOM = "custom";
const naming = ref(false);
const open = ref(false);

const options = computed<UiSelectOption[]>(() => [
  ...(dsp.activePreset ? [] : [{ value: CUSTOM, label: "Custom", disabled: true }]),
  ...dsp.presets.map((p) => ({ value: p.id, label: p.builtin ? p.name : `${p.name} (mine)` })),
]);
const selected = computed(() => dsp.activePreset?.id ?? CUSTOM);
const userSelected = computed(() => dsp.activePreset && !dsp.activePreset.builtin);
const title = computed(() =>
  unsupported.value
    ? "EQ is not supported for this stream type."
    : dsp.eqEnabled
      ? `EQ: ${dsp.activePreset?.name ?? "Custom"}`
      : "EQ off",
);

function choose(v: string | null): void {
  if (v && v !== CUSTOM) void dsp.applyPreset(v);
}
function openGraph(): void {
  open.value = false;
  graph.value = true;
}
</script>

<template>
  <PopoverRoot v-model:open="open">
    <PopoverTrigger as-child>
      <UiButton
        variant="icon"
        :size="props.size"
        :pressed="dsp.eqEnabled && !unsupported"
        :class="unsupported ? 'opacity-45' : ''"
        :title="title"
        aria-label="Equalizer"
        :disabled="disabled"
        data-testid="eq-button"
      >
        <SlidersVertical />
      </UiButton>
    </PopoverTrigger>
    <PopoverPortal>
      <PopoverContent
        side="top"
        align="end"
        :side-offset="8"
        class="z-50 w-[260px] rounded-lg border border-line bg-raised p-3 shadow-xl"
        data-testid="eq-popover"
      >
        <p v-if="unsupported" class="m-0 mb-2 text-xs text-dim" data-testid="eq-unsupported">
          EQ is not supported for this stream type.
        </p>
        <div :class="unsupported ? 'pointer-events-none opacity-40 grayscale' : ''" :inert="unsupported || undefined">
        <div class="mb-3 flex items-center justify-between">
          <span class="text-sm font-semibold">Equalizer</span>
          <UiSwitch :model-value="dsp.eqEnabled" aria-label="EQ enabled" @update:model-value="(v) => dsp.saveEqEnabled(v)" />
        </div>
        <div class="flex items-center gap-1.5">
          <UiSelect
            class="flex-1"
            aria-label="EQ preset"
            trigger-class="flex-1"
            :model-value="selected"
            :options="options"
            @update:model-value="choose"
          />
          <UiButton
            v-if="userSelected"
            variant="icon-danger"
            title="Delete this preset"
            aria-label="Delete preset"
            @click="dsp.deleteUserPreset(dsp.activePreset!.id)"
          >
            <Trash2 />
          </UiButton>
        </div>
        <div class="mt-3 flex items-center justify-between text-xs">
          <UiButton :disabled="dsp.activeBands.length === 0" data-testid="save-preset" @click="naming = true">
            Save as preset…
          </UiButton>
        </div>
        <div class="mt-2 text-right text-xs">
          <button type="button" class="border-0 bg-transparent p-0 text-dim underline hover:text-accent" data-testid="open-graph" @click="openGraph">
            Open graph…
          </button>
        </div>
        <p v-if="dsp.activeBands.length === 0" class="m-0 mt-2 text-[11px] text-faint">
          Flat: no bands. Pick a preset, or open the graph to make your own.
        </p>
        </div>
      </PopoverContent>
    </PopoverPortal>
  </PopoverRoot>
  <EqDialog v-model:open="graph" />
  <PromptDialog
    v-model:open="naming"
    title="Save EQ preset"
    label="Preset name"
    placeholder="My tuning"
    confirm-label="Save"
    :maxlength="40"
    @submit="(n) => dsp.saveUserPreset(n)"
  />
</template>
