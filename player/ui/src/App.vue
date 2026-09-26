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
import { useAnalogStore } from "./stores/analog";
import { useToastsStore } from "./stores/toasts";
import { describeAnalog } from "./types";
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
const analog = useAnalogStore();
const jobs = useJobsStore();
const toasts = useToastsStore();

// Cleanup must be registered synchronously: inside the async onMounted below
// there is no active component instance left after the first await.
let stopWatch: (() => void) | undefined;
onUnmounted(() => {
  stopWatch?.();
  player.dispose();
  window.removeEventListener("keydown", onKeydown);
});

onMounted(async () => {
  window.addEventListener("keydown", onKeydown);
  await settings.init(); // get_server_url + persisted playback prefs, then point the REST client at it
  await player.init(); // subscribe to player-state events
  await dsp.init(); // persisted EQ/loudness + device/DoP capability
  await analog.init(); // the A/B pair of analog-warmth settings
  jobs.init(); // pick up any active server jobs (scan / ISO extraction)
  // Keep the queue store in sync with core-driven queue changes.
  stopWatch = player.$subscribe((_m, s) => {
    if (s.raw) {
      void queue.syncFromState(s.raw);
      analog.noteLevel(s.raw.analog_level);
    }
  });
  // The launch state (a restored queue) arrived during player.init(), before
  // the watcher existed, and an idle engine sends nothing after it.
  if (player.raw) void queue.syncFromState(player.raw);
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
    ab: (which) => switchAnalog(which),
  });
}

/** A / B / X: swap the analog-warmth slot from anywhere, and say what is playing. */
let abToast: number | null = null;
function switchAnalog(which: "a" | "b" | "toggle"): void {
  if (!analog.loaded) return;
  if (which === "toggle") analog.toggle();
  else analog.select(which);
  if (abToast !== null) toasts.dismiss(abToast);
  abToast = toasts.push("info", `Analog warmth: listening to ${analog.active.toUpperCase()}`, {
    detail: describeAnalog(analog.current),
    ttl: 2500,
  });
}
</script>

<template>
  <div class="flex h-full flex-col">
    <div v-if="!settings.loaded" class="flex h-full items-center justify-center text-faint">Starting…</div>
    <template v-else>
      <div
        v-if="lib.serverOnline === false"
        class="border-b border-line bg-warn/15 px-4 py-1.5 text-xs text-warn-fg"
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
