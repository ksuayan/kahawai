<script setup lang="ts">
import { computed, ref } from "vue";
import { viewBoxClass } from "../lib/scrollers";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import AudiobookDetail from "./AudiobookDetail.vue";
import AudiobooksView from "./AudiobooksView.vue";
import NowPlayingBar from "./NowPlayingBar.vue";
import NowPlayingSheet from "./NowPlayingSheet.vue";
import PhoneLibraryView from "./PhoneLibraryView.vue";
import PhoneTabBar from "./PhoneTabBar.vue";
import QueueView from "./QueueView.vue";
import SearchView from "./SearchView.vue";
import SettingsView from "./SettingsView.vue";

const nav = useNavStore();
const LIBRARY_VIEWS = ["albums", "artists", "genres", "playlists", "album", "artist", "genre", "playlist"];
/** The library has its own section strip and scroll box inside (PhoneLibraryView). */
const isLibrary = computed(() => LIBRARY_VIEWS.includes(nav.view.name) || !["audiobooks", "audiobook", "search", "queue", "settings"].includes(nav.view.name));
const player = usePlayerStore();

/** The now-playing sheet. Opens from the mini-player; not a nav route on phones. */
const sheetOpen = ref(false);

function onMiniExpand(): void {
  if (player.currentTrack) sheetOpen.value = true;
}
</script>

<template>
  <!-- pt-safe: the screens start below the status bar (the app draws edge to edge). -->
  <div class="pt-safe flex h-full flex-col" data-testid="phone-shell">
    <!-- A virtualized view needs a bounded flex column; the rest scroll here (lib/scrollers). -->
    <div class="min-h-0 flex-1" :class="isLibrary ? 'flex flex-col overflow-hidden' : viewBoxClass(nav.view.name)" data-testid="phone-view-box">
      <PhoneLibraryView v-if="LIBRARY_VIEWS.includes(nav.view.name)" />
      <AudiobooksView v-else-if="nav.view.name === 'audiobooks'" />
      <AudiobookDetail v-else-if="nav.view.name === 'audiobook'" :id="nav.view.id ?? 0" />
      <SearchView v-else-if="nav.view.name === 'search'" />
      <QueueView v-else-if="nav.view.name === 'queue'" />
      <SettingsView v-else-if="nav.view.name === 'settings'" />
      <PhoneLibraryView v-else />
    </div>
    <NowPlayingBar variant="mini" @expand="onMiniExpand" />
    <PhoneTabBar />
    <NowPlayingSheet :open="sheetOpen" @update:open="(v) => (sheetOpen = v)" />
  </div>
</template>
