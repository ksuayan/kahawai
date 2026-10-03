<script setup lang="ts">
import { reactive, ref, watch } from "vue";
import { SPEEDS, speedLabel } from "../lib/audiobook";
import { usePodcastsStore } from "../stores/podcasts";
import type { PodcastFeed } from "../types";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import UiInput from "../ui/UiInput.vue";
import UiSelect from "../ui/UiSelect.vue";
import UiSwitch from "../ui/UiSwitch.vue";

/** A show's settings: how it plays (speed, skips, auto-advance) and how it downloads. */
const props = defineProps<{ open: boolean; feed: PodcastFeed }>();
const emit = defineEmits<{ (e: "update:open", v: boolean): void }>();
const podcasts = usePodcastsStore();

const form = reactive({ speed: "1", back: "15", forward: "30", autoAdvance: false, autoDownload: true, keep: "5", days: "7" });
const error = ref<string | null>(null);
const saving = ref(false);
const speedOptions = SPEEDS.map((s) => ({ value: String(s), label: speedLabel(s) }));

watch(
  () => [props.open, props.feed] as const,
  () => {
    if (!props.open) return;
    const f = props.feed;
    form.speed = String(f.speed);
    form.back = String(f.skip_back_s);
    form.forward = String(f.skip_forward_s);
    form.autoAdvance = f.auto_advance;
    form.autoDownload = f.auto_download;
    form.keep = String(f.keep_n);
    form.days = String(f.delete_played_after_days);
    error.value = null;
  },
  { immediate: true },
);

async function save(): Promise<void> {
  saving.value = true;
  error.value = null;
  try {
    await podcasts.saveSettings(props.feed.id, {
      speed: Number(form.speed),
      skip_back_s: parseInt(form.back, 10),
      skip_forward_s: parseInt(form.forward, 10),
      auto_advance: form.autoAdvance,
      auto_download: form.autoDownload,
      keep_n: parseInt(form.keep, 10),
      delete_played_after_days: parseInt(form.days, 10),
    });
    emit("update:open", false);
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <UiDialog :open="open" :title="`${feed.title}: settings`" description="Kept on the server, for every player." @update:open="(v) => emit('update:open', v)">
    <div class="grid gap-4 text-[13px]" data-testid="podcast-settings">
      <fieldset class="m-0 grid gap-2 border-0 p-0">
        <legend class="mb-1 text-xs font-semibold uppercase tracking-wide text-dim">Playing</legend>
        <label class="flex items-center justify-between gap-3">Speed <UiSelect aria-label="Speed" trigger-class="w-[90px]" :model-value="form.speed" :options="speedOptions" @update:model-value="(v) => (form.speed = v ?? '1')" /></label>
        <label class="flex items-center justify-between gap-3">Skip back (seconds) <UiInput v-model="form.back" class="w-[80px]" type="number" min="1" max="600" aria-label="Skip back seconds" /></label>
        <label class="flex items-center justify-between gap-3">Skip forward (seconds) <UiInput v-model="form.forward" class="w-[80px]" type="number" min="1" max="600" aria-label="Skip forward seconds" /></label>
        <UiSwitch v-model="form.autoAdvance" label="When an episode ends and Up Next is empty, play this show's next unplayed one" />
      </fieldset>
      <fieldset class="m-0 grid gap-2 border-0 p-0">
        <legend class="mb-1 text-xs font-semibold uppercase tracking-wide text-dim">Downloading</legend>
        <UiSwitch v-model="form.autoDownload" label="Download new episodes to the server" />
        <label class="flex items-center justify-between gap-3">Keep this many unplayed episodes <UiInput v-model="form.keep" class="w-[80px]" type="number" min="1" max="100" aria-label="Episodes to keep" /></label>
        <label class="flex items-center justify-between gap-3">Delete played files after (days, 0 = never) <UiInput v-model="form.days" class="w-[80px]" type="number" min="0" max="365" aria-label="Delete played after days" /></label>
      </fieldset>
      <p v-if="error" class="m-0 text-xs text-danger-fg" role="alert" data-testid="podcast-settings-error">{{ error }}</p>
    </div>
    <template #footer>
      <UiButton @click="emit('update:open', false)">Cancel</UiButton>
      <UiButton variant="primary" :disabled="saving" data-testid="podcast-settings-save" @click="save">Save</UiButton>
    </template>
  </UiDialog>
</template>
