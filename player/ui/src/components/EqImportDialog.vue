<script setup lang="ts">
import { TriangleAlert } from "lucide-vue-next";
import { computed, ref, watch } from "vue";
import { parseAutoEq } from "../autoeq";
import { useDspStore } from "../stores/dsp";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import UiInput from "../ui/UiInput.vue";

/**
 * Import an AutoEq-style `ParametricEQ.txt` (paste or choose a file), preview
 * what it will do, then replace the EQ with it and optionally keep it as a
 * named preset. Nothing is bundled: profiles come from the user.
 */
const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "update:open", v: boolean): void }>();

const dsp = useDspStore();
const text = ref("");
const name = ref("");
const fileError = ref<string | null>(null);

watch(
  () => props.open,
  (o) => {
    if (o) {
      text.value = "";
      name.value = "";
      fileError.value = null;
    }
  },
);

const profile = computed(() => (text.value.trim() ? parseAutoEq(text.value) : null));
const invalid = computed(() => text.value.trim() !== "" && profile.value === null);
const fmt = (n: number): string => `${n > 0 ? "+" : ""}${n}`;

async function onFile(e: Event): Promise<void> {
  const f = (e.target as HTMLInputElement).files?.[0];
  if (!f) return;
  fileError.value = null;
  try {
    text.value = await f.text();
    if (!name.value) name.value = f.name.replace(/\s*ParametricEQ\.txt$/i, "").replace(/\.txt$/i, "");
  } catch {
    fileError.value = "Could not read that file.";
  }
}

async function submit(): Promise<void> {
  const p = profile.value;
  if (!p) return;
  await dsp.importProfile(p);
  if (name.value.trim()) dsp.saveUserPreset(name.value);
  emit("update:open", false);
}
</script>

<template>
  <UiDialog
    :open="open"
    title="Import EQ profile"
    description="Paste or choose an AutoEq ParametricEQ.txt (or Equalizer APO) file. It replaces the current EQ."
    wide
    @update:open="(v) => emit('update:open', v)"
  >
    <div class="grid gap-3" data-testid="eq-import">
      <label class="block text-xs text-dim">
        File
        <input type="file" accept=".txt,text/plain" class="mt-1 block text-[13px] text-fg" data-testid="eq-import-file" @change="onFile" />
      </label>
      <label class="block text-xs text-dim">
        Profile text
        <textarea
          v-model="text"
          rows="7"
          spellcheck="false"
          class="mt-1 w-full rounded-md border border-line bg-surface px-2.5 py-1.5 font-mono text-[12px] text-fg outline-none placeholder:text-faint focus:border-accent"
          placeholder="Preamp: -6.2 dB&#10;Filter 1: ON PK Fc 31 Hz Gain 5.5 dB Q 1.00"
          data-testid="eq-import-text"
        />
      </label>
      <p v-if="fileError" class="m-0 text-xs text-danger" role="alert">{{ fileError }}</p>
      <p v-if="invalid" class="m-0 text-xs text-danger" role="alert" data-testid="eq-import-invalid">
        No filters found. Expected lines like “Filter 1: ON PK Fc 100 Hz Gain 3 dB Q 1”.
      </p>

      <div v-if="profile" class="text-xs text-dim" data-testid="eq-import-preview">
        <p class="m-0 text-fg">
          {{ profile.bands.length }} {{ profile.bands.length === 1 ? "filter" : "filters" }}, preamp {{ fmt(profile.preamp_db) }} dB
        </p>
        <ul class="m-0 mt-1 max-h-32 list-none overflow-y-auto p-0 font-mono tabular-nums">
          <li v-for="(b, i) in profile.bands" :key="i">
            {{ i + 1 }}. {{ b.band_type.replace("_", " ") }} {{ b.freq }} Hz<template v-if="b.gain_db !== 0"> {{ fmt(b.gain_db) }} dB</template>
            · {{ b.band_type.endsWith("shelf") ? "slope" : "Q" }} {{ b.q }}
          </li>
        </ul>
        <p v-for="(w, i) in profile.warnings" :key="i" class="m-0 mt-1.5 flex items-start gap-1.5 text-warn-fg" role="status" data-testid="eq-import-warning">
          <TriangleAlert class="mt-px size-3.5 shrink-0" />{{ w }}
        </p>
      </div>

      <label v-if="profile" class="block text-xs text-dim">
        Save as preset (optional)
        <UiInput v-model="name" class="mt-1 w-full" type="text" :maxlength="40" placeholder="e.g. HD 600" aria-label="Preset name" data-testid="eq-import-name" />
      </label>
    </div>
    <template #footer>
      <UiButton @click="emit('update:open', false)">Cancel</UiButton>
      <UiButton variant="primary" :disabled="!profile" data-testid="eq-import-apply" @click="submit">Import</UiButton>
    </template>
  </UiDialog>
</template>
