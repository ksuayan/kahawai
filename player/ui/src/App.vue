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

function isTypingTarget(el: EventTarget | null): boolean {
  const t = el as HTMLElement | null;
  if (!t || !("tagName" in t)) return false;
  const tag = t.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || t.isContentEditable;
}

function onKeydown(e: KeyboardEvent): void {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  if (isTypingTarget(e.target)) {
    if (e.key === "Escape") (e.target as HTMLElement).blur();
    return;
  }
  switch (e.key) {
    case " ":
      e.preventDefault();
      void player.toggle();
      break;
    case "ArrowRight":
      e.preventDefault();
      void player.seekTo(player.positionMs + 10_000);
      break;
    case "ArrowLeft":
      e.preventDefault();
      void player.seekTo(player.positionMs - 10_000);
      break;
    case "ArrowUp":
      e.preventDefault();
      void player.changeVolume(Math.min(1, player.volume + 0.05));
      break;
    case "ArrowDown":
      e.preventDefault();
      void player.changeVolume(Math.max(0, player.volume - 0.05));
      break;
    case "n":
    case "N":
      void player.nextTrack();
      break;
    case "p":
    case "P":
      void player.prevTrack();
      break;
    case "f":
    case "F":
      nav.go("search");
      break;
    case "1":
      nav.go("albums");
      break;
    case "2":
      nav.go("artists");
      break;
    case "3":
      nav.go("playlists");
      break;
    case "4":
      nav.go("search");
      break;
    case "5":
      nav.go("queue");
      break;
    case "6":
      nav.go("settings");
      break;
  }
}
</script>

<template>
  <div class="app">
    <div v-if="!settings.loaded" class="boot">Starting…</div>
    <template v-else>
      <div class="offline" v-if="lib.serverOnline === false">
        Server unreachable at {{ settings.serverUrl }} — check Settings.
      </div>
      <div class="body">
        <Sidebar />
        <main class="main">
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

<style scoped>
.app {
  height: 100%;
  display: flex;
  flex-direction: column;
}

.boot {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-faint);
}

.offline {
  background: rgba(255, 159, 10, 0.14);
  color: #ffb340;
  font-size: 12px;
  padding: 6px 16px;
  border-bottom: 1px solid var(--border);
}

.body {
  flex: 1;
  display: flex;
  min-height: 0;
}

.main {
  flex: 1;
  overflow-y: auto;
  min-width: 0;
}
</style>
