<script setup lang="ts">
import { reactive, watch } from "vue";
import type { AudiobookDetail } from "../types";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import UiInput from "../ui/UiInput.vue";

/**
 * Edit a book's details by hand: for what the tags and folder names got
 * wrong, and for narrators, which no open database has. Fields you change
 * are kept across rescans.
 */
const props = defineProps<{ open: boolean; book: AudiobookDetail }>();
const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  (e: "save", edit: { title: string; author: string; narrator: string; series: string; series_index?: number; year?: number }): void;
}>();

const form = reactive({ title: "", author: "", narrator: "", series: "", seriesIndex: "", year: "" });
watch(
  () => [props.open, props.book],
  () => {
    if (!props.open) return;
    const b = props.book;
    form.title = b.title;
    form.author = b.author ?? "";
    form.narrator = b.narrator ?? "";
    form.series = b.series ?? "";
    form.seriesIndex = b.series_index != null ? String(b.series_index) : "";
    form.year = b.year != null ? String(b.year) : "";
  },
  { immediate: true },
);

function submit(): void {
  if (!form.title.trim()) return;
  const idx = parseFloat(form.seriesIndex);
  const year = parseInt(form.year, 10);
  emit("save", {
    title: form.title.trim(),
    author: form.author,
    narrator: form.narrator,
    series: form.series,
    series_index: Number.isFinite(idx) ? idx : undefined,
    year: Number.isFinite(year) ? year : undefined,
  });
  emit("update:open", false);
}
</script>

<template>
  <UiDialog :open="open" title="Edit book details" description="Changes are kept when the folder is scanned again." @update:open="(v) => emit('update:open', v)">
    <div class="grid gap-3 text-xs text-dim" data-testid="edit-book">
      <label>Title <UiInput v-model="form.title" class="mt-1 w-full" type="text" aria-label="Title" data-testid="edit-title" /></label>
      <label>Author <UiInput v-model="form.author" class="mt-1 w-full" type="text" aria-label="Author" /></label>
      <label>Narrator <UiInput v-model="form.narrator" class="mt-1 w-full" type="text" aria-label="Narrator" data-testid="edit-narrator" /></label>
      <div class="grid grid-cols-[1fr_90px_90px] gap-2">
        <label>Series <UiInput v-model="form.series" class="mt-1 w-full" type="text" aria-label="Series" /></label>
        <label>Number <UiInput v-model="form.seriesIndex" class="mt-1 w-full" type="text" aria-label="Series number" /></label>
        <label>Year <UiInput v-model="form.year" class="mt-1 w-full" type="text" aria-label="Year" /></label>
      </div>
    </div>
    <template #footer>
      <UiButton @click="emit('update:open', false)">Cancel</UiButton>
      <UiButton variant="primary" :disabled="!form.title.trim()" data-testid="edit-save" @click="submit">Save</UiButton>
    </template>
  </UiDialog>
</template>
