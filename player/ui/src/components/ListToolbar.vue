<script setup lang="ts" generic="S extends string">
import { LayoutGrid, List } from "lucide-vue-next";
import type { LayoutMode } from "../stores/viewPrefs";
import UiButton from "../ui/UiButton.vue";
import UiSelect from "../ui/UiSelect.vue";

/** A list view's controls: List/Grid (omit `layout` to hide it) and Sort (omit `sortOptions` to hide it). */
defineProps<{
  layout?: LayoutMode;
  sort?: S;
  sortOptions?: { value: S; label: string }[];
}>();
const emit = defineEmits<{
  (e: "update:layout", v: LayoutMode): void;
  (e: "update:sort", v: S): void;
}>();
</script>

<template>
  <div class="flex items-center gap-2">
    <div v-if="layout" class="flex items-center rounded-md border border-line p-0.5" role="group" aria-label="Layout">
      <UiButton
        variant="icon"
        title="List"
        aria-label="List"
        :pressed="layout === 'list'"
        @click="emit('update:layout', 'list')"
      >
        <List />
      </UiButton>
      <UiButton
        variant="icon"
        title="Grid"
        aria-label="Grid"
        :pressed="layout === 'grid'"
        @click="emit('update:layout', 'grid')"
      >
        <LayoutGrid />
      </UiButton>
    </div>
    <UiSelect
      v-if="sortOptions"
      aria-label="Sort"
      title="Sort"
      trigger-class="w-56"
      :model-value="sort ?? null"
      :options="sortOptions"
      @update:model-value="(v) => v !== null && emit('update:sort', v as S)"
    />
  </div>
</template>
