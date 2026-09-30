<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onUnmounted, ref, watch } from "vue";
import { useVirtualizer } from "@tanstack/vue-virtual";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useScrollMemoryStore } from "../stores/scrollMemory";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";
import AlbumCard from "./AlbumCard.vue";
import { computeGridLayout, rowItemIds } from "../lib/gridwindowing";

const lib = useLibraryStore();
const nav = useNavStore();
const scrollMemory = useScrollMemoryStore();
/** Where this view remembers its scroll offset (see scrollMemory.ts). */
const SCROLL_KEY = "albums";

const GAP = 16;
const MIN_CELL_WIDTH = 160;
const OVERSCAN_ROWS = 3;
// AlbumCard renders a title line + subtitle line below the square artwork
// (both `truncate`, so always exactly one line each) — this must cover that
// text block or the virtualizer's fixed row height clips/overlaps it. Keep
// in sync with AlbumCard.vue's `mt-2` + text sizes.
const TEXT_BLOCK_HEIGHT = 44;

const scrollEl = ref<HTMLElement | null>(null);
// Seeded non-zero (rather than 0) so the grid renders its first rows
// immediately: a real ResizeObserver callback corrects this to the actual
// width within a frame, but happy-dom's stub never fires one at all, so
// this is also what every test measures against.
const containerWidth = ref(1200);

const albumIds = computed(() => lib.sortedAlbums.map((a) => a.id));
const albumById = computed(() => new Map(lib.sortedAlbums.map((a) => [a.id, a])));

/** Deterministic row geometry: same-width cells, fixed lanes. Same pattern
 *  as Koa's photo grid (see gridwindowing.ts). */
const layout = computed(() =>
  computeGridLayout(
    containerWidth.value,
    albumIds.value.length,
    MIN_CELL_WIDTH,
    GAP,
    TEXT_BLOCK_HEIGHT,
  ),
);

const virtualizer = useVirtualizer(
  computed(() => ({
    count: layout.value.rowCount,
    getScrollElement: () => scrollEl.value,
    estimateSize: () => Math.max(1, layout.value.rowHeight),
    // One virtual item = one full-width row strip; the lane math lives in
    // gridwindowing.ts (the virtualizer's own `lanes` option is its masonry
    // mode — one measurement per item — which is not this layout).
    lanes: 1,
    overscan: OVERSCAN_ROWS,
    getItemKey: (index: number) => index,
    // Come back to where the user left off (the view unmounts on navigation),
    // so the first render already realizes the right rows.
    initialOffset: () => scrollMemory.get(SCROLL_KEY),
  })),
);

/** Keep the virtualizer's cached measurements in sync when a resize changes
 *  the row geometry (lane count or row height). */
watch(
  () => [layout.value.lanes, layout.value.rowHeight] as const,
  () => virtualizer.value.measure(),
);

/** The albums rendered by virtual row `rowIndex`. */
function rowAlbums(rowIndex: number) {
  const ids = rowItemIds(rowIndex, layout.value.lanes, albumIds.value);
  return ids
    .map((id) => albumById.value.get(id))
    .filter((a): a is NonNullable<typeof a> => a !== undefined);
}

function subtitle(a: { artist?: string | null; year?: number | null; track_count: number }): string {
  const parts: string[] = [];
  if (a.artist) parts.push(a.artist);
  if (a.year) parts.push(String(a.year));
  parts.push(`${a.track_count} track${a.track_count === 1 ? "" : "s"}`);
  return parts.join(" · ");
}

const resizeObs = new ResizeObserver((entries) => {
  const w = entries[0]?.contentRect.width ?? 0;
  if (w > 0 && w !== containerWidth.value) containerWidth.value = w;
});
onUnmounted(() => resizeObs.disconnect());

// `scrollEl` is bound on the `v-else` (loaded) branch: if the library is
// still loading when this component mounts, it's null at mount time — a
// one-shot `observe()` in onMounted would never fire, since nothing would
// ever retry once the grid actually appears. The grid would then stay
// pinned at containerWidth's seeded default forever, never reflecting the
// real window size. Watching it instead attaches the observer whenever the
// grid actually appears, including well after mount.
watch(
  scrollEl,
  (el, prevEl) => {
    if (prevEl) resizeObs.unobserve(prevEl);
    if (el) {
      resizeObs.observe(el);
      // Put the scrollbar back once the sized inner strip is in the DOM (the
      // browser clamps scrollTop to the content height, so it must exist).
      void nextTick(() => {
        const saved = scrollMemory.get(SCROLL_KEY);
        if (saved > 0 && scrollEl.value === el) el.scrollTop = saved;
      });
    }
  },
  { immediate: true },
);

// Remember where the grid was scrolled to. Not while the grid is absent
// (still loading): that would overwrite the saved offset with nothing.
onBeforeUnmount(() => {
  if (scrollEl.value) scrollMemory.set(SCROLL_KEY, scrollEl.value.scrollTop);
});
</script>

<template>
  <ViewShell title="Albums" :subtitle="`${lib.albums.length} albums in library`" width="full">
    <StateMessage v-if="lib.loading" kind="loading">Loading albums…</StateMessage>
    <StateMessage v-else-if="lib.error" kind="error">{{ lib.error }}</StateMessage>
    <StateMessage v-else-if="lib.sortedAlbums.length === 0" kind="empty">No albums found.</StateMessage>
    <!--
      Virtualized grid: the DOM contains only visible rows plus overscan,
      regardless of library size — same pattern as Koa's PhotoGrid.vue
      (@tanstack/vue-virtual + the pure layout math in gridwindowing.ts).
      Rows are absolutely positioned by the virtualizer; each row is a CSS
      grid strip of `lanes` AlbumCards.

      This div is its own scroll container (not the shared `<main>` every
      other view relies on): the virtualizer needs a scroll element with
      nothing else sharing it, or its scroll-offset-to-row-index math would
      be thrown off by whatever precedes it (the ViewShell header) in a
      jointly-scrolled ancestor. ViewShell's `width="full"` makes the shell
      a bounded flex column for exactly this reason.
    -->
    <div v-else ref="scrollEl" class="min-h-0 flex-1 overflow-y-auto">
      <div :style="{ height: `${virtualizer.getTotalSize()}px`, width: '100%', position: 'relative' }">
        <div
          v-for="row in virtualizer.getVirtualItems()"
          :key="String(row.key)"
          :style="{
            position: 'absolute',
            top: 0,
            left: 0,
            width: '100%',
            transform: `translateY(${row.start}px)`,
          }"
        >
          <div
            class="grid"
            :style="{
              gridTemplateColumns: `repeat(${layout.lanes}, minmax(0, 1fr))`,
              gap: `${GAP}px`,
              marginBottom: `${GAP}px`,
            }"
          >
            <AlbumCard
              v-for="album in rowAlbums(row.index)"
              :key="album.id"
              :album="album"
              :subtitle="subtitle(album)"
              @open="(id) => nav.go('album', id)"
            />
          </div>
        </div>
      </div>
    </div>
  </ViewShell>
</template>
