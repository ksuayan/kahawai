<script setup lang="ts" generic="T">
import { computed, ref, watch } from "vue";
import { useVirtualizer } from "@tanstack/vue-virtual";
import { useOwnScrollMemory } from "../lib/ownScroll";
import { useScrollMemoryStore } from "../stores/scrollMemory";

/**
 * Virtualized list of fixed-height rows: only the visible rows (plus
 * overscan) are in the DOM. Its own scroll container, like VirtualGrid.
 * Emits `near-end` when the rendered rows reach the last few items, for
 * loading the next page as the user scrolls.
 */
const props = withDefaults(
  defineProps<{
    items: T[];
    scrollKey: string;
    rowHeight: number;
    getKey?: (item: T, index: number) => string | number;
    /** How close to the end (in rows) counts as near it. */
    nearEndRows?: number;
  }>(),
  { nearEndRows: 20, getKey: undefined },
);
const emit = defineEmits<{ (e: "near-end"): void }>();
defineSlots<{
  item(props: { item: T; index: number }): unknown;
  /** Drawn over the rows, in their coordinates (drag outlines, drop markers). */
  overlay?(): unknown;
  footer?(): unknown;
}>();

const scrollEl = ref<HTMLElement | null>(null);
const scrollMemory = useScrollMemoryStore();

const virtualizer = useVirtualizer(
  computed(() => ({
    count: props.items.length,
    getScrollElement: () => scrollEl.value,
    estimateSize: () => props.rowHeight,
    overscan: 8,
    getItemKey: (index: number) =>
      props.getKey && props.items[index] !== undefined ? props.getKey(props.items[index], index) : index,
    initialOffset: () => scrollMemory.get(props.scrollKey),
  })),
);

watch(
  () => {
    const rows = virtualizer.value.getVirtualItems();
    const last = rows.length > 0 ? rows[rows.length - 1].index : -1;
    return props.items.length > 0 && last >= props.items.length - props.nearEndRows;
  },
  (near) => near && emit("near-end"),
  { immediate: true },
);

useOwnScrollMemory(scrollEl, () => props.scrollKey);
</script>

<template>
  <div ref="scrollEl" class="min-h-0 flex-1 overflow-y-auto" data-scroller>
    <div :style="{ height: `${virtualizer.getTotalSize()}px`, width: '100%', position: 'relative' }" data-rows>
      <div
        v-for="row in virtualizer.getVirtualItems()"
        :key="String(row.key)"
        :style="{
          position: 'absolute',
          top: 0,
          left: 0,
          width: '100%',
          height: `${rowHeight}px`,
          transform: `translateY(${row.start}px)`,
        }"
      >
        <slot name="item" :item="items[row.index]" :index="row.index" />
      </div>
      <slot name="overlay" />
    </div>
    <slot name="footer" />
  </div>
</template>
