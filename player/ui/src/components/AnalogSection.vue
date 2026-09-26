<script setup lang="ts">
import { computed, ref } from "vue";
import { useAnalogStore, type Slot } from "../stores/analog";
import { usePlayerStore } from "../stores/player";
import {
  ANALOG_FLAVOURS,
  ANTI_ALIAS_CHOICES,
  FLAVOUR_INFO,
  LISTENING_RECIPES,
  type AnalogFlavour,
  type AnalogSettings,
  type AntiAliasChoice,
} from "../types";
import UiButton from "../ui/UiButton.vue";
import UiHint from "../ui/UiHint.vue";
import UiSelect, { type UiSelectOption } from "../ui/UiSelect.vue";
import UiSlider from "../ui/UiSlider.vue";
import UiSwitch from "../ui/UiSwitch.vue";
import SettingsSection from "./SettingsSection.vue";

/**
 * Analog warmth: tube and transistor character on the shared PCM output, with
 * an A/B switch. Each slot holds a complete set of settings; "Listening to"
 * chooses which one plays. Edits to the slot you are hearing apply live.
 */
const analog = useAnalogStore();
const player = usePlayerStore();

const unsupported = computed(() => player.isExclusive);
const slots: Slot[] = ["a", "b"];

const flavourOptions: UiSelectOption[] = ANALOG_FLAVOURS.map((f) => ({ value: f, label: FLAVOUR_INFO[f].label }));

const aliasLabel: Record<AntiAliasChoice, string> = {
  auto: "Auto (by sample rate)",
  x1: "1x, no protection",
  x1_adaa: "1x + ADAA",
  x2: "2x oversampling",
  x2_adaa: "2x oversampling + ADAA",
  x4: "4x oversampling",
  x4_adaa: "4x oversampling + ADAA",
};
const aliasOptions: UiSelectOption[] = ANTI_ALIAS_CHOICES.map((c) => ({ value: c, label: aliasLabel[c] }));

const pct = (v: number): number => Math.round(v * 100);

function summary(s: AnalogSettings): string {
  if (!s.enabled) return "Off (dry signal)";
  return `${FLAVOUR_INFO[s.flavour].short} · drive ${pct(s.drive)}% · mix ${pct(s.mix)}%`;
}

const appliedRecipe = ref<string | null>(null);
function useRecipe(id: string): void {
  const r = LISTENING_RECIPES.find((x) => x.id === id);
  if (!r) return;
  analog.applyRecipe(r);
  appliedRecipe.value = r.title;
}

const status = computed(() => (player.analogPlan ? `Now playing with ${player.analogPlan}.` : "The stage is off, or nothing is playing on the shared output."));
</script>

<template>
  <SettingsSection title="Analog warmth (experimental)">
    <UiHint>
      Adds the character of a tube or transistor stage to the shared PCM output, after the EQ. Choose from several tubes (12AX7, 12AT7, 12AU7, 6SN7, 6DJ8, 300B, 2A3), a push-pull pair, or solid-state stages; choosing a flavour also sets Sag and Transformer to typical values for it. Set up two versions and
      switch between them while music plays: compare the effect against the dry signal, or one flavour or
      anti-aliasing plan against another. Changes fade in without clicks. It does not apply to DoP or bit-perfect
      output.
    </UiHint>
    <p v-if="unsupported" class="m-0 mb-2 text-sm text-dim" role="status" data-testid="analog-unsupported">
      Analog warmth is not supported for this stream type.
    </p>

    <div :class="unsupported ? 'pointer-events-none opacity-40 grayscale' : ''" :inert="unsupported || undefined" data-testid="analog-editor">
      <div class="mb-3 flex flex-wrap items-center gap-3" role="group" aria-label="Listening to">
        <span class="text-dim">Listening to</span>
        <div class="inline-flex overflow-hidden rounded-md border border-line">
          <button
            v-for="s in slots"
            :key="s"
            type="button"
            class="min-w-[72px] border-0 px-4 py-1.5 font-semibold transition-colors"
            :class="analog.active === s ? 'bg-accent text-white' : 'bg-surface text-dim hover:bg-hover hover:text-fg'"
            :aria-pressed="analog.active === s"
            :data-testid="`ab-${s}`"
            @click="analog.select(s)"
          >
            {{ s.toUpperCase() }}
          </button>
        </div>
        <UiButton data-testid="ab-toggle" @click="analog.toggle()">Switch A/B</UiButton>
      </div>
      <p class="m-0 mb-3 text-xs text-dim" data-testid="analog-status">{{ status }}</p>

      <div class="grid grid-cols-1 gap-3 min-[900px]:grid-cols-2">
        <div
          v-for="s in slots"
          :key="s"
          class="rounded-lg border bg-surface p-3"
          :class="analog.active === s ? 'border-accent' : 'border-line'"
          :data-testid="`slot-${s}`"
        >
          <div class="mb-2 flex items-center justify-between gap-2">
            <span class="heading-3">{{ s.toUpperCase() }}</span>
            <span class="truncate text-xs text-dim" :data-testid="`slot-${s}-summary`">{{ summary(analog[s]) }}</span>
          </div>

          <UiSwitch
            :model-value="analog[s].enabled"
            :label="`Warmth on (${s.toUpperCase()})`"
            :aria-label="`Warmth on in ${s.toUpperCase()}`"
            @update:model-value="(v) => analog.update(s, { enabled: v })"
          />

          <div class="mt-3 flex flex-col gap-3" :class="!analog[s].enabled && 'opacity-50'">
            <label class="flex flex-col gap-1 text-dim">
              Flavour
              <UiSelect
                :aria-label="`Flavour ${s.toUpperCase()}`"
                trigger-class="w-full"
                :model-value="analog[s].flavour"
                :options="flavourOptions"
                @update:model-value="(v) => analog.setFlavour(s, v as AnalogFlavour)"
              />
              <span class="text-xs text-faint" :data-testid="`flavour-${s}-blurb`">{{ FLAVOUR_INFO[analog[s].flavour].blurb }}</span>
            </label>

            <div class="flex flex-col gap-1 text-dim">
              <div class="flex justify-between"><span>Drive</span><span class="tabular-nums">{{ pct(analog[s].drive) }}%</span></div>
              <UiSlider
                :aria-label="`Drive ${s.toUpperCase()}`"
                :model-value="pct(analog[s].drive)"
                :min="0"
                :max="100"
                :step="1"
                @update:model-value="(v) => analog.update(s, { drive: v / 100 })"
              />
            </div>

            <div class="flex flex-col gap-1 text-dim">
              <div class="flex justify-between"><span>Mix</span><span class="tabular-nums">{{ pct(analog[s].mix) }}%</span></div>
              <UiSlider
                :aria-label="`Mix ${s.toUpperCase()}`"
                :model-value="pct(analog[s].mix)"
                :min="0"
                :max="100"
                :step="1"
                @update:model-value="(v) => analog.update(s, { mix: v / 100 })"
              />
            </div>

            <div class="flex flex-col gap-1 text-dim">
              <div class="flex justify-between"><span>Sag</span><span class="tabular-nums">{{ pct(analog[s].sag) }}%</span></div>
              <UiSlider
                :aria-label="`Sag ${s.toUpperCase()}`"
                :model-value="pct(analog[s].sag)"
                :min="0"
                :max="100"
                :step="1"
                title="Loud passages lower the stage's headroom and gain a little, and it recovers over about a tenth of a second."
                @update:model-value="(v) => analog.update(s, { sag: v / 100 })"
              />
            </div>

            <div class="flex flex-col gap-1 text-dim">
              <div class="flex justify-between"><span>Transformer</span><span class="tabular-nums">{{ pct(analog[s].transformer) }}%</span></div>
              <UiSlider
                :aria-label="`Transformer ${s.toUpperCase()}`"
                :model-value="pct(analog[s].transformer)"
                :min="0"
                :max="100"
                :step="1"
                title="The bass saturates as the level rises, adding bass harmonics. Mids and highs are untouched."
                @update:model-value="(v) => analog.update(s, { transformer: v / 100 })"
              />
            </div>

            <div class="flex flex-col gap-1 text-dim">
              <div class="flex justify-between">
                <span>Output</span>
                <span class="tabular-nums">{{ analog[s].output_db > 0 ? "+" : "" }}{{ analog[s].output_db.toFixed(1) }} dB</span>
              </div>
              <UiSlider
                :aria-label="`Output ${s.toUpperCase()}`"
                :model-value="analog[s].output_db"
                :min="-6"
                :max="6"
                :step="0.5"
                @update:model-value="(v) => analog.update(s, { output_db: v })"
              />
            </div>

            <UiSwitch
              :model-value="analog[s].auto_gain"
              :label="`Match level to the dry signal (${s.toUpperCase()})`"
              :aria-label="`Match level ${s.toUpperCase()}`"
              @update:model-value="(v) => analog.update(s, { auto_gain: v })"
            />

            <label class="flex flex-col gap-1 text-dim">
              Anti-aliasing
              <UiSelect
                :aria-label="`Anti-aliasing ${s.toUpperCase()}`"
                trigger-class="w-full"
                :model-value="analog[s].antialias"
                :options="aliasOptions"
                @update:model-value="(v) => analog.update(s, { antialias: v as AntiAliasChoice })"
              />
            </label>
          </div>

          <div class="mt-3">
            <UiButton :data-testid="`copy-${s}`" @click="analog.copy(s, s === 'a' ? 'b' : 'a')">
              Copy {{ s.toUpperCase() }} to {{ s === "a" ? "B" : "A" }}
            </UiButton>
          </div>
        </div>
      </div>
      <div class="mt-5" data-testid="recipes">
        <h4 class="heading-3 m-0 mb-1">Listening suggestions</h4>
        <p class="prose-text mb-2 mt-0 text-dim">
          Ready-made comparisons. Choose one to load it into A and B, play the suggested kind of music, and switch. Match the
          level with the Output slider first: the louder side always sounds better.
        </p>
        <p v-if="appliedRecipe" class="m-0 mb-2 text-xs text-ok" role="status" data-testid="recipe-applied">
          Loaded "{{ appliedRecipe }}" into A and B. You are listening to A.
        </p>
        <div class="flex flex-col gap-2">
          <details v-for="r in LISTENING_RECIPES" :key="r.id" class="rounded-lg border border-line bg-surface" :data-testid="`recipe-${r.id}`">
            <summary class="cursor-pointer select-none px-3 py-2 font-semibold">{{ r.title }}</summary>
            <div class="flex flex-col gap-1.5 px-3 pb-3 text-dim">
              <p class="m-0">{{ r.idea }}</p>
              <p class="m-0"><span class="text-fg">Play:</span> {{ r.play }}</p>
              <p class="m-0"><span class="text-fg">Listen for:</span> {{ r.listen }}</p>
              <div><UiButton :data-testid="`use-${r.id}`" @click="useRecipe(r.id)">Set up A and B</UiButton></div>
            </div>
          </details>
        </div>
      </div>
      <UiHint spaced tone="faint">
        Level match uses a −12 dBFS sine as its reference, so loud music will not be perfectly level-matched. Trust your
        ears, and use Output to even out what you hear.
      </UiHint>
    </div>
  </SettingsSection>
</template>
