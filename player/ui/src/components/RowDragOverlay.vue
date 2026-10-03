<script setup lang="ts">
import { ROW_PART } from "../lib/rowDrag";

/**
 * Drawn over a VirtualList's rows while one is dragged (lib/rowDrag): a dashed
 * outline where the row is now, and a tinted, dashed gap with a bright line
 * where it will land.
 */
defineProps<{ dragFrom: number | null; dropSlot: number | null; rowHeight: number }>();
</script>

<template>
  <div
    v-if="dragFrom !== null"
    class="pointer-events-none absolute inset-x-0 z-10 rounded-lg border-2 border-dashed border-dim/70"
    :style="{ top: `${dragFrom * rowHeight + 1}px`, height: `${rowHeight - 2}px` }"
    data-testid="drag-outline"
  />
  <div
    v-if="dropSlot !== null"
    class="pointer-events-none absolute inset-x-1 z-10 flex flex-col justify-center rounded border border-dashed border-accent/70 bg-accent/15 transition-all duration-100 ease-out"
    :style="{ top: `${dropSlot * rowHeight - ROW_PART - 1}px`, height: `${2 * ROW_PART + 2}px` }"
    data-testid="drop-slot"
    :data-slot="dropSlot"
  >
    <div class="mx-1 h-0.5 rounded-full bg-accent" />
  </div>
</template>
