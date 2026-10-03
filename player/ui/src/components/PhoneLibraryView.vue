<script setup lang="ts">
import { computed } from "vue";
import { viewBoxClass } from "../lib/scrollers";
import { SECTION_LABELS, useNavStore, type ViewName } from "../stores/nav";
import AlbumDetail from "./AlbumDetail.vue";
import AlbumsView from "./AlbumsView.vue";
import ArtistDetail from "./ArtistDetail.vue";
import ArtistsView from "./ArtistsView.vue";
import GenreDetail from "./GenreDetail.vue";
import GenresView from "./GenresView.vue";
import PlaylistDetail from "./PlaylistDetail.vue";
import PlaylistsView from "./PlaylistsView.vue";

const nav = useNavStore();

const segments: { name: ViewName; label: string }[] = [
  { name: "albums", label: SECTION_LABELS.albums },
  { name: "artists", label: SECTION_LABELS.artists },
  { name: "genres", label: SECTION_LABELS.genres },
  { name: "playlists", label: SECTION_LABELS.playlists },
];

/** Detail pages keep their trail; the segment follows the trail's section. */
const active = computed<ViewName>(() => {
  const s = nav.section;
  return (segments.some((x) => x.name === s) ? s : "albums") as ViewName;
});

const view = computed(() => nav.view);
</script>

<template>
  <div class="flex h-full flex-col" data-testid="phone-library">
    <div class="shrink-0 border-b border-line bg-raised px-3 pb-2 pt-2" role="tablist" aria-label="Library sections">
      <div class="grid grid-cols-4 gap-1 rounded-lg bg-surface p-1">
        <button
          v-for="s in segments"
          :key="s.name"
          type="button"
          role="tab"
          :aria-selected="active === s.name"
          class="min-h-[44px] rounded-md border-0 bg-transparent px-2 text-[13px] font-semibold text-dim"
          :class="{ 'bg-active text-fg': active === s.name }"
          :data-testid="`lib-tab-${s.name}`"
          @click="nav.go(s.name)"
        >
          {{ s.label }}
        </button>
      </div>
    </div>
    <!-- A virtualized view needs a bounded flex column; the rest scroll here (lib/scrollers). -->
    <div class="min-h-0 flex-1" :class="viewBoxClass(view.name)" data-testid="phone-library-box">
      <AlbumsView v-if="view.name === 'albums'" />
      <AlbumDetail v-else-if="view.name === 'album'" :id="view.id ?? 0" />
      <ArtistsView v-else-if="view.name === 'artists'" />
      <ArtistDetail v-else-if="view.name === 'artist'" :id="view.id ?? 0" />
      <GenresView v-else-if="view.name === 'genres'" />
      <GenreDetail v-else-if="view.name === 'genre'" :name="view.genre ?? ''" />
      <PlaylistsView v-else-if="view.name === 'playlists'" />
      <PlaylistDetail v-else-if="view.name === 'playlist'" :id="view.id ?? 0" />
      <AlbumsView v-else />
    </div>
  </div>
</template>
