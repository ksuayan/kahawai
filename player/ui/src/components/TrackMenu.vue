<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "reka-ui";
import { useJobsStore } from "../stores/jobs";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import { isPlayable, trackTitle, type Track } from "../types";
import PromptDialog from "../ui/PromptDialog.vue";
import UiButton from "../ui/UiButton.vue";

/**
 * Reusable "⋯" action menu for a track or a track list (album).
 * Actions: play next, add to queue, add to playlist, extract SACD ISO.
 */
const props = withDefaults(
  defineProps<{
    /** Single track context. */
    track?: Track | null;
    /** Multi-track context (album). `track` wins when both are given. */
    tracks?: Track[] | null;
    albumId?: number | null;
  }>(),
  { track: null, tracks: null, albumId: null },
);

const queue = useQueueStore();
const playlists = usePlaylistsStore();
const jobs = useJobsStore();
const toasts = useToastsStore();

const busy = ref(false);
const naming = ref(false); // "New playlist…" dialog

const list = computed<Track[]>(() => {
  if (props.track) return [props.track];
  return (props.tracks ?? []).filter(isPlayable);
});

const canExtractIso = computed(
  () => props.track?.format === "sacd_iso" && !props.track.missing,
);

onMounted(() => {
  // One shared fetch no matter how many menus mount; the store dedups.
  if (!playlists.loaded) void playlists.load();
});

async function playNext(): Promise<void> {
  try {
    await queue.playNext(list.value);
    toasts.push("success", `Playing next: ${list.value.length} track${list.value.length === 1 ? "" : "s"}`);
  } catch (e) {
    toasts.push("error", "Play next failed", { detail: e instanceof Error ? e.message : String(e) });
  }
}

async function addToQueue(): Promise<void> {
  try {
    await queue.appendTracks(list.value);
    toasts.push("success", `Added ${list.value.length} track${list.value.length === 1 ? "" : "s"} to queue`);
  } catch (e) {
    toasts.push("error", "Add to queue failed", { detail: e instanceof Error ? e.message : String(e) });
  }
}

async function addToPlaylist(id: number): Promise<void> {
  busy.value = true;
  try {
    if (props.albumId != null && !props.track) {
      await playlists.addAlbum(id, props.albumId);
    } else {
      await playlists.addTracks(id, list.value.map((t) => t.id));
    }
  } catch (e) {
    toasts.push("error", "Add to playlist failed", { detail: e instanceof Error ? e.message : String(e) });
  } finally {
    busy.value = false;
  }
}

async function createAndAdd(name: string): Promise<void> {
  busy.value = true;
  try {
    const pl = await playlists.create(name);
    await addToPlaylist(pl.id);
  } catch (e) {
    toasts.push("error", "Could not create playlist", { detail: e instanceof Error ? e.message : String(e) });
  } finally {
    busy.value = false;
  }
}

function extractIso(): void {
  if (props.track) void jobs.extractIso(props.track);
}

const itemClass =
  "flex cursor-default select-none items-center justify-between gap-3 whitespace-nowrap rounded-md px-2.5 py-2 text-[13px] text-fg outline-none " +
  "data-[disabled]:opacity-40 data-[highlighted]:bg-hover data-[state=open]:bg-hover";
const contentClass =
  "z-[60] max-h-80 min-w-[200px] overflow-y-auto rounded-[10px] border border-line bg-raised p-1 shadow-[0_8px_24px_rgba(0,0,0,0.5)]";

const menuLabel = computed(() => {
  const t = props.track;
  return t ? `Actions for ${trackTitle(t)}` : "Actions";
});
</script>

<template>
  <span class="inline-block" @dblclick.stop>
    <DropdownMenuRoot>
      <DropdownMenuTrigger as-child>
        <UiButton variant="icon" :title="menuLabel" :aria-label="menuLabel">⋯</UiButton>
      </DropdownMenuTrigger>
      <DropdownMenuPortal>
        <DropdownMenuContent align="end" :side-offset="4" :class="contentClass">
          <DropdownMenuItem :class="itemClass" :disabled="list.length === 0 || busy" @select="playNext">
            ⏭ Play next
          </DropdownMenuItem>
          <DropdownMenuItem :class="itemClass" :disabled="list.length === 0 || busy" @select="addToQueue">
            ☰ Add to queue
          </DropdownMenuItem>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger :class="itemClass" :disabled="list.length === 0 || busy">
              ♫ Add to playlist… <span class="text-faint">▸</span>
            </DropdownMenuSubTrigger>
            <DropdownMenuPortal>
              <DropdownMenuSubContent :class="contentClass" :side-offset="6">
                <DropdownMenuItem
                  v-for="p in playlists.items"
                  :key="p.id"
                  :class="itemClass"
                  :disabled="busy"
                  @select="addToPlaylist(p.id)"
                >
                  {{ p.name }} <span class="text-[11px] text-faint">({{ p.track_ids.length }})</span>
                </DropdownMenuItem>
                <DropdownMenuSeparator v-if="playlists.items.length" class="my-1 h-px bg-line" />
                <DropdownMenuItem :class="itemClass" :disabled="busy" @select="naming = true">
                  ＋ New playlist…
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuPortal>
          </DropdownMenuSub>
          <DropdownMenuItem v-if="canExtractIso" :class="itemClass" :disabled="busy" @select="extractIso">
            💿 Extract to DSF
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
    <PromptDialog
      v-model:open="naming"
      title="New playlist"
      label="Playlist name"
      placeholder="Playlist name"
      confirm-label="Create and add"
      @submit="createAndAdd"
    />
  </span>
</template>
