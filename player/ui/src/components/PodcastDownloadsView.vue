<script setup lang="ts">
import { LoaderCircle, Trash2 } from "lucide-vue-next";
import { computed, onMounted } from "vue";
import { episodeDate, sizeText } from "../lib/podcast";
import { useNavStore } from "../stores/nav";
import { usePodcastsStore } from "../stores/podcasts";
import StateMessage from "../ui/StateMessage.vue";
import UiButton from "../ui/UiButton.vue";
import UiInput from "../ui/UiInput.vue";
import UiSwitch from "../ui/UiSwitch.vue";
import ViewShell from "../ui/ViewShell.vue";

/**
 * What the server has downloaded: the folder and how much it holds, what is
 * downloading now, every stored episode with its size, and each show's rules
 * (download new episodes, how many to keep, when played files go).
 */
const podcasts = usePodcastsStore();
const nav = useNavStore();

onMounted(() => {
  void podcasts.loadDownloads();
  if (!podcasts.loaded) void podcasts.loadFeeds();
});

const active = computed(() => [...podcasts.downloadJobs.entries()]);

async function setRule(feedId: number, key: "auto_download" | "keep_n" | "delete_played_after_days", value: boolean | string): Promise<void> {
  const v = typeof value === "boolean" ? value : parseInt(value, 10);
  if (typeof v === "number" && !Number.isFinite(v)) return;
  try {
    await podcasts.saveSettings(feedId, { [key]: v });
  } catch (e) {
    podcasts.error = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <ViewShell width="wide" section="podcasts" crumb="Downloads">
    <p v-if="podcasts.folder" class="m-0 mb-4 text-[13px] text-dim" data-testid="downloads-folder">
      {{ podcasts.folder.episodes_downloaded }} episodes, {{ sizeText(podcasts.folder.bytes_downloaded) || "0 KB" }}, in
      <span class="select-text font-mono text-xs">{{ podcasts.folder.path }}</span>
      <span v-if="!podcasts.folder.usable" class="text-warn-fg"> (can't be written to: choose another folder in the Server app's Settings)</span>
    </p>
    <p v-if="podcasts.error" class="m-0 mb-3 text-xs text-danger-fg" role="alert">{{ podcasts.error }}</p>

    <section v-if="active.length" class="mb-6" aria-label="Downloading">
      <h3 class="heading-3 mb-2">Downloading</h3>
      <ul class="m-0 list-none p-0">
        <li v-for="[id, job] in active" :key="id" class="flex items-center gap-3 py-1 text-[13px]" data-testid="download-active">
          <LoaderCircle class="size-4 shrink-0 animate-spin text-dim" />
          <span class="min-w-0 flex-1 truncate">{{ job.label.replace(/^Download: /, "") }}</span>
          <span class="w-12 text-right tabular-nums text-dim">{{ Math.round(job.progress * 100) }}%</span>
        </li>
      </ul>
    </section>

    <section class="mb-6" aria-label="Downloaded episodes">
      <h3 class="heading-3 mb-2">Downloaded</h3>
      <StateMessage v-if="podcasts.downloads.length === 0" kind="empty">Nothing downloaded. Episodes still play: they stream from the podcast.</StateMessage>
      <ul v-else class="m-0 list-none p-0">
        <li v-for="e in podcasts.downloads" :key="e.id" class="flex items-center gap-3 rounded-md px-2 py-1.5 hover:bg-hover" data-testid="download-row">
          <button type="button" class="min-w-0 flex-1 border-0 bg-transparent p-0 text-left" @click="nav.go('episode', e.id)">
            <span class="block truncate font-semibold text-fg">{{ e.title }}</span>
            <span class="block truncate text-xs text-dim">{{ e.feed_title }} · {{ episodeDate(e.published_at) }}<template v-if="e.played_at"> · played</template></span>
          </button>
          <span class="w-20 shrink-0 text-right text-xs tabular-nums text-dim" data-testid="download-size">{{ sizeText(e.file_bytes) }}</span>
          <UiButton variant="icon-danger" title="Delete the file (the episode stays)" aria-label="Delete the download" data-testid="download-delete" @click="podcasts.removeDownload(e)"><Trash2 /></UiButton>
        </li>
      </ul>
    </section>

    <section aria-label="Download rules">
      <h3 class="heading-3 mb-2">Rules for each show</h3>
      <StateMessage v-if="podcasts.feeds.length === 0" kind="empty">No shows yet.</StateMessage>
      <table v-else class="w-full border-collapse text-[13px]" data-testid="download-rules">
        <thead>
          <tr class="text-left text-xs text-dim">
            <th class="py-1 font-semibold">Show</th>
            <th class="py-1 font-semibold">Download new</th>
            <th class="py-1 font-semibold">Keep unplayed</th>
            <th class="py-1 font-semibold">Delete played after (days)</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="f in podcasts.feeds" :key="f.id" class="border-t border-line" data-testid="download-rule">
            <td class="max-w-[260px] truncate py-1.5 pr-3">{{ f.title }}</td>
            <td class="py-1.5"><UiSwitch :model-value="f.auto_download" :aria-label="`Download new episodes of ${f.title}`" @update:model-value="(v) => setRule(f.id, 'auto_download', v)" /></td>
            <td class="py-1.5"><UiInput class="w-[70px]" type="number" min="1" max="100" :model-value="String(f.keep_n)" :aria-label="`Episodes of ${f.title} to keep`" @change="(e: Event) => setRule(f.id, 'keep_n', (e.target as HTMLInputElement).value)" /></td>
            <td class="py-1.5"><UiInput class="w-[70px]" type="number" min="0" max="365" :model-value="String(f.delete_played_after_days)" :aria-label="`Days before played episodes of ${f.title} are deleted`" @change="(e: Event) => setRule(f.id, 'delete_played_after_days', (e.target as HTMLInputElement).value)" /></td>
          </tr>
        </tbody>
      </table>
    </section>
  </ViewShell>
</template>
