<script setup lang="ts" generic="T">
import type { LayoutMode } from "../stores/viewPrefs";
import { useLibraryStore } from "../stores/library";
import type { Track } from "../types";
import TrackCard from "./TrackCard.vue";
import TrackRow from "./TrackRow.vue";
import VirtualGrid from "./VirtualGrid.vue";
import VirtualList from "./VirtualList.vue";

/**
 * A track list shown as a virtualized list or grid (the view's List/Grid
 * choice). Items can be tracks or wrappers around them (a queue entry and
 * its position): `track` reads the track out. The list row is a TrackRow
 * unless the `row` slot supplies one; the grid shows TrackCards. It is its
 * own scroll container, so the view must be a ViewShell `width="full"`.
 */
const props = withDefaults(
  defineProps<{
    items: T[];
    track: (item: T) => Track;
    layout: LayoutMode;
    scrollKey: string;
    getKey?: (item: T, index: number) => string | number;
    isCurrent?: (item: T) => boolean;
    /** Row number in list mode, badge in grid mode. */
    number?: (item: T, index: number) => number | null;
    rowHeight?: number;
  }>(),
  { getKey: undefined, isCurrent: () => false, number: () => null, rowHeight: 52 },
);
const emit = defineEmits<{ (e: "play", item: T): void; (e: "near-end"): void }>();
defineSlots<{ row?(props: { item: T; index: number }): unknown; overlay?(): unknown; footer?(): unknown }>();

const lib = useLibraryStore();
const key = (item: T, index: number) => (props.getKey ? props.getKey(item, index) : props.track(item).id);
</script>

<template>
  <VirtualGrid
    v-if="layout === 'grid'"
    :items="items"
    :scroll-key="`${scrollKey}:grid`"
    :get-key="key"
    :min-cell-width="150"
    @near-end="emit('near-end')"
  >
    <template #item="{ item, index }">
      <TrackCard
        :track="track(item)"
        :artwork-hash="lib.artworkFor(track(item))"
        :current="isCurrent(item)"
        :badge="number(item, index)"
        @play="emit('play', item)"
      />
    </template>
  </VirtualGrid>
  <VirtualList
    v-else
    :items="items"
    :scroll-key="scrollKey"
    :row-height="rowHeight"
    :get-key="key"
    @near-end="emit('near-end')"
  >
    <template #item="{ item, index }">
      <slot name="row" :item="item" :index="index">
        <TrackRow
          :track="track(item)"
          :number="number(item, index)"
          :show-artwork="true"
          :artwork-hash="lib.artworkFor(track(item))"
          :current="isCurrent(item)"
          @play="emit('play', item)"
        />
      </slot>
    </template>
    <template #overlay><slot name="overlay" /></template>
    <template #footer><slot name="footer" /></template>
  </VirtualList>
</template>
