<script setup lang="ts" generic="T extends { id: number }">
import { computed, onUnmounted, ref, watch } from "vue";
import { useVirtualizer } from "@tanstack/vue-virtual";
import { computeGridLayout, rowItemIds } from "../lib/gridwindowing";
import { useOwnScrollMemory } from "../lib/ownScroll";
import { useScrollMemoryStore } from "../stores/scrollMemory";

/**
 * Virtualized card grid: the DOM holds only the visible rows plus overscan,
 * however many items there are (same pattern as Koa's PhotoGrid:
 * @tanstack/vue-virtual + the layout math in gridwindowing.ts). Rows are
 * absolutely positioned strips of `lanes` cards. It is its own scroll
 * container: the virtualizer's offset math needs a scroller nothing else
 * shares, so the parent must be a bounded flex column (ViewShell
 * `width="full"`).
 */
const props = withDefaults(
  defineProps<{
    items: T[];
    /** Where the scroll offset is remembered (scrollMemory). */
    scrollKey: string;
    minCellWidth?: number;
    /** Height of what a card shows below its square artwork. */
    textBlockHeight?: number;
    gap?: number;
  }>(),
  { minCellWidth: 160, textBlockHeight: 44, gap: 16 },
);
const emit = defineEmits<{ (e: "near-end"): void }>();
defineSlots<{ item(props: { item: T }): unknown }>();

const OVERSCAN_ROWS = 3;

const scrollEl = ref<HTMLElement | null>(null);
// Seeded non-zero so the first rows render at once: a real ResizeObserver
// corrects it within a frame (happy-dom's stub never fires, so tests
// measure against this).
const containerWidth = ref(1200);
const scrollMemory = useScrollMemoryStore();

const ids = computed(() => props.items.map((i) => i.id));
const byId = computed(() => new Map(props.items.map((i) => [i.id, i])));
const layout = computed(() =>
  computeGridLayout(containerWidth.value, ids.value.length, props.minCellWidth, props.gap, props.textBlockHeight),
);

const virtualizer = useVirtualizer(
  computed(() => ({
    count: layout.value.rowCount,
    getScrollElement: () => scrollEl.value,
    estimateSize: () => Math.max(1, layout.value.rowHeight),
    lanes: 1,
    overscan: OVERSCAN_ROWS,
    getItemKey: (index: number) => index,
    initialOffset: () => scrollMemory.get(props.scrollKey),
  })),
);

watch(
  () => [layout.value.lanes, layout.value.rowHeight] as const,
  () => virtualizer.value.measure(),
);

function rowItems(rowIndex: number): T[] {
  return rowItemIds(rowIndex, layout.value.lanes, ids.value)
    .map((id) => byId.value.get(id))
    .filter((i): i is T => i !== undefined);
}

// Lazy loading: say so when the rendered rows reach the end.
watch(
  () => {
    const rows = virtualizer.value.getVirtualItems();
    return rows.length > 0 && rows[rows.length - 1].index >= layout.value.rowCount - 1 - OVERSCAN_ROWS;
  },
  (near) => near && emit("near-end"),
  { immediate: true },
);

const resizeObs = new ResizeObserver((entries) => {
  const w = entries[0]?.contentRect.width ?? 0;
  if (w > 0 && w !== containerWidth.value) containerWidth.value = w;
});
onUnmounted(() => resizeObs.disconnect());
watch(
  scrollEl,
  (el, prev) => {
    if (prev) resizeObs.unobserve(prev);
    if (el) resizeObs.observe(el);
  },
  { immediate: true },
);
useOwnScrollMemory(scrollEl, () => props.scrollKey);
</script>

<template>
  <div ref="scrollEl" class="min-h-0 flex-1 overflow-y-auto">
    <div :style="{ height: `${virtualizer.getTotalSize()}px`, width: '100%', position: 'relative' }">
      <div
        v-for="row in virtualizer.getVirtualItems()"
        :key="String(row.key)"
        :style="{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${row.start}px)` }"
      >
        <div
          class="grid"
          :style="{
            gridTemplateColumns: `repeat(${layout.lanes}, minmax(0, 1fr))`,
            gap: `${gap}px`,
            marginBottom: `${gap}px`,
          }"
        >
          <template v-for="item in rowItems(row.index)" :key="item.id">
            <slot name="item" :item="item" />
          </template>
        </div>
      </div>
    </div>
  </div>
</template>
