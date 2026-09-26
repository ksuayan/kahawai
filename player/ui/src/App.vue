<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import AlbumsView from "./components/AlbumsView.vue";
import AlbumDetail from "./components/AlbumDetail.vue";
import ArtistsView from "./components/ArtistsView.vue";
import ArtistDetail from "./components/ArtistDetail.vue";
import NowPlayingBar from "./components/NowPlayingBar.vue";
import NowPlayingView from "./components/NowPlayingView.vue";
import PlaylistsView from "./components/PlaylistsView.vue";
import PlaylistDetail from "./components/PlaylistDetail.vue";
import QueueView from "./components/QueueView.vue";
import SearchView from "./components/SearchView.vue";
import SettingsView from "./components/SettingsView.vue";
import Sidebar from "./components/Sidebar.vue";
import ToastHost from "./components/ToastHost.vue";
import { useJobsStore } from "./stores/jobs";
import { useLibraryStore } from "./stores/library";
import { useNavStore } from "./stores/nav";
import { useDspStore } from "./stores/dsp";
import { usePlayerStore } from "./stores/player";
import { usePlaylistsStore } from "./stores/playlists";
import { useQueueStore } from "./stores/queue";
import { useSettingsStore } from "./stores/settings";
import { handleShortcut } from "./shortcuts";

const nav = useNavStore();
const settings = useSettingsStore();
const lib = useLibraryStore();
const player = usePlayerStore();
const queue = useQueueStore();
const playlists = usePlaylistsStore();
const dsp = useDspStore();
const jobs = useJobsStore();

onMounted(async () => {
  await settings.init(); // get_server_url + persisted playback prefs, then point the REST client at it
  await player.init(); // subscribe to player-state events
  await dsp.init(); // persisted EQ/loudness + device/DoP capability
  jobs.init(); // pick up any active server jobs (scan / ISO extraction)
  // Keep the queue store in sync with core-driven queue changes.
  const stopWatch = player.$subscribe((_m, s) => {
    if (s.raw) void queue.syncFromState(s.raw);
  });
  window.addEventListener("keydown", onKeydown);
  onUnmounted(() => {
    stopWatch();
    player.dispose();
    window.removeEventListener("keydown", onKeydown);
  });
  await Promise.all([lib.loadAll(), playlists.load()]);
});

function onKeydown(e: KeyboardEvent): void {
  handleShortcut(e, {
    toggle: () => void player.toggle(),
    seekBy: (d) => void player.seekTo(player.positionMs + d),
    volumeBy: (d) => void player.changeVolume(Math.min(1, Math.max(0, player.volume + d))),
    next: () => void player.nextTrack(),
    prev: () => void player.prevTrack(),
    go: (v) => nav.go(v),
  });
}
</script>

<template>
  <div class="flex h-full flex-col">
    <div v-if="!settings.loaded" class="flex h-full items-center justify-center text-faint">Starting…</div>
    <template v-else>
      <div
        v-if="lib.serverOnline === false"
        class="border-b border-line bg-[rgba(255,159,10,0.14)] px-4 py-1.5 text-xs text-[#ffb340]"
        role="alert"
      >
        Server unreachable at {{ settings.serverUrl }} — check Settings.
      </div>
      <div class="flex min-h-0 flex-1">
        <Sidebar />
        <main class="min-w-0 flex-1 overflow-y-auto">
          <AlbumsView v-if="nav.view.name === 'albums'" />
          <AlbumDetail v-else-if="nav.view.name === 'album'" :id="nav.view.id ?? 0" />
          <ArtistsView v-else-if="nav.view.name === 'artists'" />
          <ArtistDetail v-else-if="nav.view.name === 'artist'" :id="nav.view.id ?? 0" />
          <NowPlayingView v-else-if="nav.view.name === 'nowplaying'" />
          <PlaylistsView v-else-if="nav.view.name === 'playlists'" />
          <PlaylistDetail v-else-if="nav.view.name === 'playlist'" :id="nav.view.id ?? 0" />
          <SearchView v-else-if="nav.view.name === 'search'" />
          <QueueView v-else-if="nav.view.name === 'queue'" />
          <SettingsView v-else-if="nav.view.name === 'settings'" />
        </main>
      </div>
      <NowPlayingBar />
      <ToastHost />
    </template>
  </div>
</template>
