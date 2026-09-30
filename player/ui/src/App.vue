<script setup lang="ts">
import { onMounted, onUnmounted, provide, ref } from "vue";
import AlbumsView from "./components/AlbumsView.vue";
import AlbumDetail from "./components/AlbumDetail.vue";
import ArtistsView from "./components/ArtistsView.vue";
import GenreDetail from "./components/GenreDetail.vue";
import GenresView from "./components/GenresView.vue";
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
import AboutDialog from "./components/AboutDialog.vue";
import { useAbxStore } from "./stores/abx";
import { useAnalogStore } from "./stores/analog";
import { useOverlaysStore } from "./stores/overlays";
import { useToastsStore } from "./stores/toasts";
import { describeAnalog } from "./types";
import { MAIN_SCROLL } from "./lib/mainScroll";
import { useDspStore } from "./stores/dsp";
import { usePlayerStore } from "./stores/player";
import { usePlaylistsStore } from "./stores/playlists";
import { useQueueStore } from "./stores/queue";
import { useSettingsStore } from "./stores/settings";
import { useServerHealthStore } from "./stores/serverHealth";
import { handleShortcut } from "./shortcuts";
import { onMenuAction } from "./tauri";
import { onCatalogUpdated, onServerConnected } from "./api";

const nav = useNavStore();

// The shared scroller most views live in; views remember their place in it.
const mainEl = ref<HTMLElement | null>(null);
provide(MAIN_SCROLL, mainEl);
const settings = useSettingsStore();
const lib = useLibraryStore();
const player = usePlayerStore();
const queue = useQueueStore();
const playlists = usePlaylistsStore();
const dsp = useDspStore();
const analog = useAnalogStore();
const jobs = useJobsStore();
const toasts = useToastsStore();
const abx = useAbxStore();
const overlays = useOverlaysStore();
const serverHealth = useServerHealthStore();
/** Views that scroll inside a virtualized list or grid of their own. */
const OWN_SCROLLER = new Set(["albums", "artists", "genre"]);

// Cleanup must be registered synchronously: inside the async onMounted below
// there is no active component instance left after the first await.
let stopWatch: (() => void) | undefined;
let stopMenu: (() => void) | undefined;
// Registered synchronously (not inside onMounted) for the same reason as
// stopMenu below: unsubscribing must be reachable from onUnmounted even
// though the subscription itself is set up after an await.
const stopCatalogEvents = onCatalogUpdated(() => void lib.loadAll());
// The library and playlists load once at launch. If the server wasn't up yet
// (it started after the player, or the NAS is still booting), retry them as
// soon as the event stream connects. Only after a failed load: a healthy
// reconnect doesn't refetch the whole catalog.
const stopReloadOnConnect = onServerConnected(() => {
  if (lib.error) void lib.loadAll();
  if (playlists.error) void playlists.reload();
});
const stopServerHealth = serverHealth.init();
onUnmounted(() => {
  stopWatch?.();
  stopMenu?.();
  stopCatalogEvents();
  stopReloadOnConnect();
  stopServerHealth();
  player.dispose();
  window.removeEventListener("keydown", onKeydown);
});

onMounted(async () => {
  window.addEventListener("keydown", onKeydown);
  // Native menu (macOS): the shell forwards item ids; the UI owns what they do.
  stopMenu = (await onMenuAction((id) => {
    if (id === "app.about") overlays.openAbout();
  })) ?? undefined;
  await settings.init(); // get_server_url + persisted playback prefs, then point the REST client at it (and (re)connects /api/events)
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
let abxToast: number | null = null;
function switchAnalog(which: "a" | "b" | "toggle"): void {
  if (!analog.loaded) return;
  if (abx.running) {
    // Blind test: X is the third choice, and nothing may say which slot is playing.
    const h = which === "toggle" ? "x" : which;
    abx.hear(h);
    if (abxToast !== null) toasts.dismiss(abxToast);
    abxToast = toasts.push("info", `Blind test: hearing ${h.toUpperCase()}`, { ttl: 1500 });
    return;
  }
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
        <template v-if="lib.showingCached">
          Offline: showing your cached library. Playback needs the server at {{ settings.serverUrl }}.
        </template>
        <template v-else>Server unreachable at {{ settings.serverUrl }} — check Settings.</template>
      </div>
      <div class="flex min-h-0 flex-1">
        <Sidebar />
        <!--
          Virtualized views (Albums, Artists, a genre's tracks) have their own scroller,
          so <main> must not scroll for them: two nested scrollbars (and a scroll
          position split between them) is what you get otherwise. There, <main> is a
          plain flex column the view fills.
        -->
        <main
          ref="mainEl"
          class="min-w-0 flex-1"
          :class="OWN_SCROLLER.has(nav.view.name) ? 'flex flex-col overflow-hidden' : 'overflow-y-auto'"
        >
          <AlbumsView v-if="nav.view.name === 'albums'" />
          <AlbumDetail v-else-if="nav.view.name === 'album'" :id="nav.view.id ?? 0" />
          <ArtistsView v-else-if="nav.view.name === 'artists'" />
          <ArtistDetail v-else-if="nav.view.name === 'artist'" :id="nav.view.id ?? 0" />
          <GenresView v-else-if="nav.view.name === 'genres'" />
          <GenreDetail v-else-if="nav.view.name === 'genre'" :name="nav.view.genre ?? ''" />
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
      <AboutDialog />
    </template>
  </div>
</template>
