<script setup lang="ts">
import { ref } from "vue";
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
const player = usePlayerStore();

/** The now-playing sheet. Opens from the mini-player; not a nav route on phones. */
const sheetOpen = ref(false);

function onMiniExpand(): void {
  if (player.currentTrack) sheetOpen.value = true;
}
</script>

<template>
  <div class="flex h-full flex-col" data-testid="phone-shell">
    <div class="min-h-0 flex-1">
      <PhoneLibraryView v-if="['albums', 'artists', 'genres', 'playlists', 'album', 'artist', 'genre', 'playlist'].includes(nav.view.name)" />
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
