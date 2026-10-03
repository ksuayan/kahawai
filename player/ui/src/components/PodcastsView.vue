<script setup lang="ts">
import { ArrowDown, ArrowUp, Download, FileDown, FileUp, ListEnd, Play, Plus, RefreshCw, TriangleAlert, X } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { podcastOpmlExportUrl } from "../api";
import { useNavStore } from "../stores/nav";
import { usePodcastsStore } from "../stores/podcasts";
import { useViewPrefsStore } from "../stores/viewPrefs";
import { openUrl } from "../tauri";
import ConfirmDialog from "../ui/ConfirmDialog.vue";
import PromptDialog from "../ui/PromptDialog.vue";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import ViewShell from "../ui/ViewShell.vue";
import Artwork from "./Artwork.vue";
import EpisodeRow from "./EpisodeRow.vue";
import ListToolbar from "./ListToolbar.vue";
import PodcastCard from "./PodcastCard.vue";
import PodcastContextMenu from "./PodcastContextMenu.vue";
import PodcastSettingsDialog from "./PodcastSettingsDialog.vue";
import type { PodcastFeed } from "../types";

/**
 * Podcasts: Up Next, the episodes you are part-way through, and your shows
 * (a grid or a list, with unplayed counts and failing feeds marked). Add a
 * show by its feed address, or bring your subscriptions over as OPML.
 */
const podcasts = usePodcastsStore();
const nav = useNavStore();
const view = useViewPrefsStore();

const adding = ref(false);
/** The show whose settings (from its menu) are open, or that is about to be deleted. */
const settingsFor = ref<PodcastFeed | null>(null);
const deleting = ref<PodcastFeed | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);

onMounted(() => void podcasts.loadFeeds());

async function onFile(e: Event): Promise<void> {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  await podcasts.importOpml(await file.text());
}

async function add(url: string): Promise<void> {
  const feed = await podcasts.subscribe(url);
  if (feed) nav.go("podcast", feed.id);
}
</script>

<template>
  <ViewShell title="Podcasts" :subtitle="`${podcasts.feeds.length} ${podcasts.feeds.length === 1 ? 'show' : 'shows'}`" width="wide">
    <template #actions>
      <UiButton variant="primary" data-testid="podcast-add" @click="adding = true"><Plus /> Add podcast</UiButton>
      <UiButton title="Import an OPML file from another podcast app" data-testid="podcast-import" @click="fileInput?.click()"><FileUp /> Import OPML</UiButton>
      <UiButton title="Save your subscriptions as an OPML file" :disabled="podcasts.feeds.length === 0" data-testid="podcast-export" @click="openUrl(podcastOpmlExportUrl())"><FileDown /> Export OPML</UiButton>
      <UiButton variant="icon" title="Check every show for new episodes" aria-label="Refresh all" :disabled="podcasts.feeds.length === 0" data-testid="podcast-refresh-all" @click="podcasts.refresh()"><RefreshCw /></UiButton>
      <UiButton variant="icon" title="Downloads" aria-label="Downloads" data-testid="podcast-downloads" @click="nav.go('podcastdownloads')"><Download /></UiButton>
      <ListToolbar v-model:layout="view.prefs.podcastsLayout" />
      <input ref="fileInput" type="file" accept=".opml,.xml,text/xml,text/x-opml,application/xml" class="hidden" data-testid="podcast-import-file" @change="onFile" />
    </template>

    <StateMessage v-if="podcasts.loading && !podcasts.loaded" kind="loading">Loading podcasts…</StateMessage>
    <StateMessage v-else-if="podcasts.error && !podcasts.feeds.length" kind="error">{{ podcasts.error }}</StateMessage>
    <StateMessage v-else-if="podcasts.feeds.length === 0" kind="empty">
      No podcasts yet. Add one by its feed address, or import an OPML file from another podcast app.
    </StateMessage>
    <template v-else>
      <section v-if="podcasts.upNext.length" class="mb-6" aria-label="Up Next" data-testid="up-next">
        <h3 class="heading-3 mb-2 flex items-center gap-2"><ListEnd class="size-4" /> Up Next</h3>
        <ol class="m-0 list-none p-0">
          <li v-for="(e, i) in podcasts.upNext" :key="e.id" class="flex items-center gap-3 rounded-md px-2 py-1.5 hover:bg-hover" data-testid="up-next-row">
            <span class="w-5 shrink-0 text-right text-xs tabular-nums text-faint">{{ i + 1 }}</span>
            <Artwork :url="e.image_url || e.feed_image_url" placeholder="podcast" :size="36" :radius="4" :alt="e.feed_title" />
            <button type="button" class="min-w-0 flex-1 border-0 bg-transparent p-0 text-left" @click="nav.go('episode', e.id)">
              <span class="block truncate font-semibold text-fg">{{ e.title }}</span>
              <span class="block truncate text-xs text-dim">{{ e.feed_title }}</span>
            </button>
            <UiButton variant="icon" title="Play now" aria-label="Play now" @click="podcasts.play(e)"><Play class="fill-current" /></UiButton>
            <UiButton variant="icon" title="Move up" aria-label="Move up" :disabled="i === 0" @click="podcasts.moveInUpNext(e.id, -1)"><ArrowUp /></UiButton>
            <UiButton variant="icon" title="Move down" aria-label="Move down" :disabled="i === podcasts.upNext.length - 1" @click="podcasts.moveInUpNext(e.id, 1)"><ArrowDown /></UiButton>
            <UiButton variant="icon" title="Remove from Up Next" aria-label="Remove from Up Next" data-testid="up-next-remove" @click="podcasts.removeFromUpNext(e.id)"><X /></UiButton>
          </li>
        </ol>
      </section>

      <section v-if="podcasts.inProgress.length" class="mb-6" aria-label="Continue listening" data-testid="podcast-continue">
        <h3 class="heading-3 mb-2">Continue listening</h3>
        <ul class="m-0 list-none p-0">
          <EpisodeRow v-for="e in podcasts.inProgress.slice(0, 5)" :key="e.id" :episode="e" show-feed show-art />
        </ul>
      </section>

      <h3 class="heading-3 mb-2">Shows</h3>
      <div v-if="view.prefs.podcastsLayout === 'grid'" class="grid grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-4" data-testid="podcast-grid">
        <PodcastContextMenu v-for="f in podcasts.feeds" :key="f.id" :feed="f" @settings="(x) => (settingsFor = x)" @delete="(x) => (deleting = x)">
          <PodcastCard :feed="f" @open="(id) => nav.go('podcast', id)" />
        </PodcastContextMenu>
      </div>
      <ul v-else class="m-0 list-none p-0" data-testid="podcast-list">
        <li v-for="f in podcasts.feeds" :key="f.id">
          <PodcastContextMenu :feed="f" @settings="(x) => (settingsFor = x)" @delete="(x) => (deleting = x)">
          <button type="button" class="flex w-full items-center gap-3 rounded-md border-0 bg-transparent px-2.5 py-1.5 text-left hover:bg-hover" data-testid="podcast-row" @click="nav.go('podcast', f.id)">
            <Artwork :url="f.image_url" placeholder="podcast" :size="48" :radius="4" :alt="f.title" />
            <span class="min-w-0 flex-1">
              <span class="block truncate font-semibold text-fg">{{ f.title }}</span>
              <span class="block truncate text-xs text-dim">{{ f.author || f.feed_url }}</span>
            </span>
            <span v-if="f.last_error" class="flex shrink-0 items-center gap-1 text-xs text-danger-fg" :title="f.last_error" data-testid="podcast-failing"><TriangleAlert class="size-3.5" /> Can't be read</span>
            <span class="w-24 shrink-0 text-right text-xs tabular-nums text-faint">{{ f.unplayed_count }} unplayed</span>
          </button>
          </PodcastContextMenu>
        </li>
      </ul>
    </template>

    <PodcastSettingsDialog v-if="settingsFor" :open="settingsFor !== null" :feed="podcasts.feedById(settingsFor.id) ?? settingsFor" @update:open="(v) => !v && (settingsFor = null)" />
    <ConfirmDialog
      :open="deleting !== null"
      :title="`Delete ${deleting?.title ?? 'this show'}?`"
      description="You unsubscribe: its episodes, places and downloaded files are removed from the server."
      confirm-label="Delete"
      danger
      @update:open="(v) => !v && (deleting = null)"
      @confirm="deleting && podcasts.unsubscribe(deleting.id)"
    />
    <PromptDialog
      v-model:open="adding"
      title="Add a podcast"
      label="The podcast's feed address (RSS)"
      placeholder="https://…"
      confirm-label="Subscribe"
      @submit="add"
    />
  </ViewShell>
</template>
