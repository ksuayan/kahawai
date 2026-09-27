<script setup lang="ts">
import { TriangleAlert } from "lucide-vue-next";
import { computed, ref } from "vue";
import { useAbxStore } from "../stores/abx";
import { useAnalogStore, type Slot } from "../stores/analog";
import { usePlayerStore } from "../stores/player";
import {
  ANALOG_FLAVOURS,
  ANTI_ALIAS_CHOICES,
  describeAnalog,
  FLAVOUR_INFO,
  LISTENING_RECIPES,
  type AnalogFlavour,
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
const abx = useAbxStore();
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

const summary = describeAnalog;

const appliedRecipe = ref<string | null>(null);
function useRecipe(id: string): void {
  const r = LISTENING_RECIPES.find((x) => x.id === id);
  if (!r) return;
  analog.applyRecipe(r);
  appliedRecipe.value = r.title;
}

// --- level meter ---------------------------------------------------------
const level = computed(() => player.analogLevel);
const dB = (v: number, d = 1): string => `${v > 0 ? "+" : ""}${v.toFixed(d)}`;
/** Position of the marker on the −6…+6 dB scale, as a percentage. */
const markerPct = computed(() => (level.value ? Math.min(100, Math.max(0, ((level.value.delta_db + 6) / 12) * 100)) : 50));
const meterState = computed<"ok" | "warn" | "bad">(() => {
  const d = Math.abs(level.value?.delta_db ?? 0);
  return d < 0.5 ? "ok" : d < 1.5 ? "warn" : "bad";
});
const clipping = computed(() => (level.value?.peak_dbfs ?? -99) > -0.3);
const meterMessage = computed(() => {
  if (!analog.current.enabled) return "Slot " + analog.active.toUpperCase() + " is the dry signal: nothing to measure. Its level change is 0 dB by definition.";
  if (!player.isPlaying) return "Play music on the shared output to measure.";
  return "Measuring… (a couple of seconds of audio)";
});
const measuredText = (s: Slot): string => {
  const m = analog.measured[s];
  return m === null ? "not measured yet" : `${dB(m)} dB${analog[s].enabled ? "" : " (dry)"}`;
};
const canMatch = computed(() => analog.measured.a !== null && analog.measured.b !== null);
const matchNote = ref<string | null>(null);
function match(from: Slot, to: Slot): void {
  const r = analog.matchLevel(from, to);
  if (!r) return;
  matchNote.value =
    r.changed_db === 0
      ? `${to.toUpperCase()} is already level with ${from.toUpperCase()}.`
      : `Set the Output of ${to.toUpperCase()} to ${dB(analog[to].output_db)} dB (${dB(r.changed_db)} dB)${r.clamped ? "; that is the limit of the slider, so it may still differ" : ""}. Play a few seconds to confirm.`;
}

// --- blind test -------------------------------------------------------------
const trialOptions: UiSelectOption[] = [5, 10, 15, 20].map((n) => ({ value: String(n), label: String(n) }));
const trialCount = ref(10);
const startAnyway = ref(false);
const canStartBlind = computed(() => abx.slotsDiffer && (abx.levelMatched || startAnyway.value));
const blindCheck = computed(() => {
  if (!abx.slotsDiffer) return "A and B are identical: change one of them first.";
  if (abx.levelDifference === null) return "Measure both slots first (play music with A, then with B) so the levels can be matched.";
  if (!abx.levelMatched) return `The levels differ by ${abx.levelDifference.toFixed(1)} dB. Use Match B to A first: a louder side gives itself away.`;
  return `Levels are matched (within ${abx.levelDifference.toFixed(1)} dB).`;
});
const heardLabel = computed(() => abx.heard.toUpperCase());

const status = computed(() => (player.analogPlan ? `Now playing with ${player.analogPlan}.` : "The stage is off, or nothing is playing on the shared output."));

// --- master on/off ---------------------------------------------------------
// One switch for the whole feature, independent of either slot's own
// `enabled` (slot A is deliberately the dry point of comparison, so it being
// "off" does not mean the feature is off). Gates every interaction below it,
// the same way an unsupported stream type does. Disabled mid-blind-test so a
// listener can't be stranded with the Cancel button unreachable.
const dimmed = computed(() => unsupported.value || !analog.masterOn);
</script>

<template>
  <SettingsSection title="Analog warmth">
    <UiSwitch
      class="mb-3"
      :model-value="analog.masterOn"
      :disabled="unsupported || abx.running"
      label="Enabled"
      data-testid="analog-master-toggle"
      :title="abx.running ? 'Finish or cancel the blind test to change this' : undefined"
      @update:model-value="analog.setMasterOn"
    />
    <UiHint>
      Adds the character of a tube or transistor stage to the shared PCM output, after the EQ. Choose from several tubes (12AX7, 12AT7, 12AU7, 6SN7, 6DJ8, 300B, 2A3), a push-pull pair, or solid-state stages; choosing a flavour also sets Sag and Transformer to typical values for it. Set up two versions and
      switch between them while music plays: compare the effect against the dry signal, or one flavour or
      anti-aliasing plan against another. Changes fade in without clicks. It does not apply to DoP or bit-perfect
      output.
    </UiHint>
    <p v-if="unsupported" class="m-0 mb-2 text-sm text-dim" role="status" data-testid="analog-unsupported">
      Analog warmth is not supported for this stream type.
    </p>
    <p v-else-if="!analog.masterOn" class="m-0 mb-2 text-sm text-dim" role="status" data-testid="analog-off">
      Analog warmth is off. Turn it on above to use the level meter, the blind test, or the A/B editors.
    </p>

    <div :class="dimmed ? 'pointer-events-none opacity-40 grayscale' : ''" :inert="dimmed || undefined" data-testid="analog-editor">
      <div v-if="!abx.running" class="mb-3 rounded-lg border border-line bg-surface p-3" data-testid="level-meter">
        <div class="mb-1 flex flex-wrap items-baseline justify-between gap-2">
          <span class="heading-3">Level meter</span>
          <span class="text-xs text-dim">what the stage does to the loudness of what you are hearing</span>
        </div>
        <template v-if="level && analog.effective.enabled">
          <div class="mb-2 flex flex-wrap items-baseline gap-x-6 gap-y-1 tabular-nums">
            <span class="text-dim">Before <span class="text-fg" data-testid="meter-in">{{ level.input_lufs.toFixed(1) }}</span> LUFS</span>
            <span class="text-dim">After <span class="text-fg" data-testid="meter-out">{{ level.output_lufs.toFixed(1) }}</span> LUFS</span>
            <span class="text-dim">
              Change
              <span
                class="font-semibold"
                :class="meterState === 'ok' ? 'text-ok' : meterState === 'warn' ? 'text-warn-fg' : 'text-danger-fg'"
                data-testid="meter-delta"
                :data-state="meterState"
              >{{ dB(level.delta_db) }} dB</span>
            </span>
            <span class="text-dim">Peak <span class="text-fg" data-testid="meter-peak">{{ level.peak_dbfs.toFixed(1) }}</span> dBFS</span>
          </div>
          <div class="relative h-2 rounded-sm bg-active" role="img" :aria-label="`Level change ${dB(level.delta_db)} dB on a scale from minus 6 to plus 6`">
            <div class="absolute left-1/2 top-0 h-full w-px bg-faint" />
            <div
              class="absolute top-[-2px] h-3 w-1.5 rounded-sm"
              :class="meterState === 'ok' ? 'bg-ok' : meterState === 'warn' ? 'bg-warn' : 'bg-danger'"
              :style="{ left: `calc(${markerPct}% - 3px)` }"
              data-testid="meter-marker"
            />
          </div>
          <div class="mt-1 flex justify-between text-[11px] text-faint tabular-nums"><span>−6 dB</span><span>0</span><span>+6 dB</span></div>
          <p v-if="clipping" class="m-0 mt-2 flex items-start gap-1.5 text-xs text-danger-fg" role="status" data-testid="meter-clip">
            <TriangleAlert class="mt-px size-3.5 shrink-0" />
            The output peaks at {{ level.peak_dbfs.toFixed(1) }} dBFS and may clip. Lower Drive or Output.
          </p>
        </template>
        <p v-else class="m-0 text-xs text-dim" data-testid="meter-idle">{{ meterMessage }}</p>

        <div class="mt-3 flex flex-wrap items-center gap-3 border-t border-line pt-3 text-xs">
          <span class="text-dim" data-testid="measured-a">A: {{ measuredText("a") }}</span>
          <span class="text-dim" data-testid="measured-b">B: {{ measuredText("b") }}</span>
          <UiButton :disabled="!canMatch" data-testid="match-b" title="Change B's Output so B is as loud as A" @click="match('a', 'b')">Match B to A</UiButton>
          <UiButton :disabled="!canMatch" data-testid="match-a" title="Change A's Output so A is as loud as B" @click="match('b', 'a')">Match A to B</UiButton>
        </div>
        <p v-if="!canMatch" class="m-0 mt-2 text-xs text-faint">
          To match levels, play music with A, then with B, for a few seconds each. Each slot's level change is remembered until you edit it.
        </p>
        <p v-if="matchNote" class="m-0 mt-2 text-xs text-dim" role="status" data-testid="match-note">{{ matchNote }}</p>
      </div>

      <div v-if="abx.running" class="mb-3 rounded-lg border border-accent bg-surface p-3" data-testid="blind-test">
        <div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
          <span class="heading-3">Blind test</span>
          <span class="text-dim tabular-nums" data-testid="blind-progress">Trial {{ abx.answered + 1 }} of {{ abx.total }}</span>
        </div>
        <p class="m-0 mb-3 text-xs text-dim">
          A and B are the two you set up. X is secretly one of them. Switch between A, B and X as often as you like (keys
          A, B and X), then say which one X is. Settings and levels are hidden until the end.
        </p>
        <div class="mb-3 flex flex-wrap items-center gap-3" role="group" aria-label="Listening to">
          <span class="text-dim">Hearing</span>
          <div class="inline-flex overflow-hidden rounded-md border border-line">
            <button
              v-for="h in (['a', 'b', 'x'] as const)"
              :key="h"
              type="button"
              class="min-w-[72px] border-0 px-4 py-1.5 font-semibold transition-colors"
              :class="abx.heard === h ? 'bg-accent text-white' : 'bg-surface text-dim hover:bg-hover hover:text-fg'"
              :aria-pressed="abx.heard === h"
              :aria-keyshortcuts="h.toUpperCase()"
              :data-testid="`blind-hear-${h}`"
              @click="abx.hear(h)"
            >
              {{ h.toUpperCase() }}
            </button>
          </div>
          <span class="text-xs text-faint" data-testid="blind-heard">Hearing {{ heardLabel }}</span>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <span class="text-dim">X is</span>
          <UiButton data-testid="blind-answer-a" @click="abx.answer('a')">A</UiButton>
          <UiButton data-testid="blind-answer-b" @click="abx.answer('b')">B</UiButton>
          <UiButton class="ml-auto" data-testid="blind-cancel" @click="abx.cancel()">Cancel test</UiButton>
        </div>
      </div>

      <div v-else-if="abx.finished" class="mb-3 rounded-lg border border-line bg-surface p-3" data-testid="blind-result">
        <span class="heading-3">Blind test result</span>
        <p class="m-0 my-2" data-testid="blind-verdict">{{ abx.verdict }}</p>
        <ol class="m-0 mb-2 list-none p-0 text-xs text-dim" data-testid="blind-answers">
          <li v-for="(t, i) in abx.revealed" :key="i" class="tabular-nums">
            Trial {{ i + 1 }}: X was {{ t.truth.toUpperCase() }}, you said {{ t.guess.toUpperCase() }}
            <span :class="t.truth === t.guess ? 'text-ok' : 'text-danger-fg'">{{ t.truth === t.guess ? "correct" : "wrong" }}</span>
          </li>
        </ol>
        <UiButton data-testid="blind-dismiss" @click="abx.dismiss()">Done</UiButton>
      </div>

      <div v-else class="mb-3 flex flex-wrap items-center gap-3 rounded-lg border border-line bg-surface p-3" data-testid="blind-start">
        <span class="heading-3">Blind test</span>
        <label class="flex items-center gap-2 text-dim">
          Trials
          <UiSelect
            aria-label="Number of trials"
            trigger-class="w-[80px]"
            :model-value="String(trialCount)"
            :options="trialOptions"
            @update:model-value="(v) => (trialCount = Number(v))"
          />
        </label>
        <UiButton variant="primary" :disabled="!canStartBlind" data-testid="blind-start-button" @click="abx.start(trialCount)">Start blind test</UiButton>
        <label v-if="abx.slotsDiffer && !abx.levelMatched" class="flex items-center gap-2 text-xs text-dim">
          <input v-model="startAnyway" type="checkbox" data-testid="blind-anyway" /> Start anyway (results will be unreliable)
        </label>
        <p class="m-0 w-full text-xs text-dim" data-testid="blind-check">{{ blindCheck }}</p>
      </div>

      <div v-if="!abx.running" class="mb-3 flex flex-wrap items-center gap-3" role="group" aria-label="Listening to">
        <span class="text-dim">Listening to</span>
        <div class="inline-flex overflow-hidden rounded-md border border-line">
          <button
            v-for="s in slots"
            :key="s"
            type="button"
            class="min-w-[72px] border-0 px-4 py-1.5 font-semibold transition-colors"
            :class="analog.active === s ? 'bg-accent text-white' : 'bg-surface text-dim hover:bg-hover hover:text-fg'"
            :aria-pressed="analog.active === s"
            :aria-keyshortcuts="s.toUpperCase()"
            :title="`Listen to ${s.toUpperCase()} (press ${s.toUpperCase()})`"
            :data-testid="`ab-${s}`"
            @click="analog.select(s)"
          >
            {{ s.toUpperCase() }}
          </button>
        </div>
        <UiButton data-testid="ab-toggle" title="Switch A/B (press X)" aria-keyshortcuts="X" @click="analog.toggle()">Switch A/B</UiButton>
        <span class="text-xs text-faint">Keys: <kbd class="font-mono">A</kbd> <kbd class="font-mono">B</kbd> <kbd class="font-mono">X</kbd>, from any screen</span>
      </div>
      <p v-if="!abx.running" class="m-0 mb-3 text-xs text-dim" data-testid="analog-status">{{ status }}</p>

      <div v-if="!abx.running" class="grid grid-cols-1 gap-3 min-[900px]:grid-cols-2" data-testid="slots">
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
      <div v-if="!abx.running" class="mt-5" data-testid="recipes">
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
