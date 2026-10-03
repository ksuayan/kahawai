<script setup lang="ts">
import { computed } from "vue";
import { SORT_OPTIONS, sortAlbums } from "../lib/sorting";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { useViewPrefsStore } from "../stores/viewPrefs";
import StateMessage from "../ui/StateMessage.vue";
import ViewShell from "../ui/ViewShell.vue";
import AlbumCard from "./AlbumCard.vue";
import Artwork from "./Artwork.vue";
import ItemContextMenu from "./ItemContextMenu.vue";
import ListToolbar from "./ListToolbar.vue";
import VirtualGrid from "./VirtualGrid.vue";
import VirtualList from "./VirtualList.vue";

const lib = useLibraryStore();
const nav = useNavStore();
const view = useViewPrefsStore();

/** List rows: 48px cover plus padding. */
const ROW_HEIGHT = 60;

const albums = computed(() => sortAlbums(lib.albums, view.prefs.albumsSort));

function subtitle(a: { artist?: string | null; year?: number | null; track_count: number }): string {
  const parts: string[] = [];
  if (a.artist) parts.push(a.artist);
  if (a.year) parts.push(String(a.year));
  parts.push(`${a.track_count} track${a.track_count === 1 ? "" : "s"}`);
  return parts.join(" · ");
}
</script>

<template>
  <ViewShell title="Albums" :subtitle="`${lib.albums.length} albums in library`" width="full">
    <template #actions>
      <ListToolbar
        v-model:layout="view.prefs.albumsLayout"
        v-model:sort="view.prefs.albumsSort"
        :sort-options="SORT_OPTIONS"
      />
    </template>
    <StateMessage v-if="lib.loading" kind="loading">Loading albums…</StateMessage>
    <StateMessage v-else-if="lib.error" kind="error">{{ lib.error }}</StateMessage>
    <StateMessage v-else-if="albums.length === 0" kind="empty">No albums found.</StateMessage>
    <VirtualGrid v-else-if="view.prefs.albumsLayout === 'grid'" :items="albums" scroll-key="albums" :min-cell-width="150">
      <template #item="{ item }">
        <AlbumCard :album="item" :subtitle="subtitle(item)" @open="(id) => nav.go('album', id)" />
      </template>
    </VirtualGrid>
    <VirtualList
      v-else
      :items="albums"
      scroll-key="albums:list"
      :row-height="ROW_HEIGHT"
      :get-key="(a) => a.id"
    >
      <template #item="{ item }">
        <ItemContextMenu :album="item">
        <button
          type="button"
          class="flex h-full w-full items-center gap-3 rounded-md px-2.5 text-left hover:bg-hover"
          data-testid="album-row"
          @click="nav.go('album', item.id)"
        >
          <Artwork :hash="item.artwork_hash" :size="48" :radius="4" :alt="item.title" />
          <div class="min-w-0 flex-1">
            <div class="truncate font-semibold">{{ item.title }}</div>
            <div class="truncate text-xs text-dim">{{ item.artist ?? "Unknown artist" }}</div>
          </div>
          <span class="w-12 shrink-0 text-right tabular-nums text-dim">{{ item.year ?? "" }}</span>
          <span class="w-20 shrink-0 text-right text-xs tabular-nums text-faint">
            {{ item.track_count }} track{{ item.track_count === 1 ? "" : "s" }}
          </span>
        </button>
        </ItemContextMenu>
      </template>
    </VirtualList>
  </ViewShell>
</template>
