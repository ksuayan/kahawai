<script setup lang="ts">
import { ChevronRight, Disc3, Info, ListMusic, ListPlus, MicVocal, Play, Plus } from "lucide-vue-next";
import { computed, ref } from "vue";
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuPortal,
  ContextMenuRoot,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "reka-ui";
import { useLibraryActions } from "../lib/libraryActions";
import { useOverlaysStore } from "../stores/overlays";
import { usePlaylistsStore } from "../stores/playlists";
import { isPlayable, type Album, type Track } from "../types";
import PromptDialog from "../ui/PromptDialog.vue";

/**
 * Right-click menu for a list row or grid card: a track or an album. Wraps
 * its single child (the row or card), which becomes the trigger. Play is
 * left to the view (`play`) for a track, so it plays the view's list from
 * there; an album plays its own tracks.
 */
const props = withDefaults(defineProps<{ track?: Track | null; album?: Album | null }>(), {
  track: null,
  album: null,
});
const emit = defineEmits<{ (e: "play"): void }>();

const actions = useLibraryActions();
const overlays = useOverlaysStore();
const playlists = usePlaylistsStore();
const naming = ref(false);

const artistName = computed(() => props.track?.artist ?? props.album?.artist ?? null);
const albumId = computed(() => props.track?.album_id ?? props.album?.id ?? null);
const hasArtist = computed(() => actions.findArtist(artistName.value) !== undefined);
const canPlay = computed(() => (props.track ? isPlayable(props.track) : props.album !== null));

/** The tracks an action applies to (an album's are fetched on demand). */
async function tracks(): Promise<Track[]> {
  if (props.track) return [props.track];
  return props.album ? actions.albumTracks(props.album) : [];
}

function onOpen(open: boolean): void {
  if (open && !playlists.loaded) void playlists.load();
}

function play(): void {
  if (props.track) emit("play");
  else if (props.album) void actions.playAlbum(props.album);
}

function info(): void {
  if (props.track) overlays.showInfo({ kind: "track", track: props.track });
  else if (props.album) overlays.showInfo({ kind: "album", album: props.album });
}

const itemClass =
  "flex cursor-default select-none items-center justify-between gap-3 whitespace-nowrap rounded-md px-2.5 py-2 text-[13px] text-fg outline-none " +
  "data-[disabled]:opacity-40 data-[highlighted]:bg-hover data-[state=open]:bg-hover";
const contentClass =
  "z-[60] max-h-80 min-w-[200px] overflow-y-auto rounded-md border border-line bg-surface p-1 shadow-float";
</script>

<template>
  <ContextMenuRoot @update:open="onOpen">
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <ContextMenuPortal>
      <ContextMenuContent :class="contentClass" data-kw-fade data-testid="item-menu">
        <ContextMenuItem :class="itemClass" :disabled="!canPlay" @select="play">
          <span class="flex items-center gap-2"><Play class="size-4 text-dim" />Play</span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" :disabled="!hasArtist" @select="actions.goToArtist(artistName)">
          <span class="flex items-center gap-2"><MicVocal class="size-4 text-dim" />Go to Artist</span>
        </ContextMenuItem>
        <ContextMenuItem :class="itemClass" :disabled="albumId === null" @select="actions.goToAlbum(albumId)">
          <span class="flex items-center gap-2"><Disc3 class="size-4 text-dim" />Go to Album</span>
        </ContextMenuItem>
        <ContextMenuSeparator class="my-1 h-px bg-line" />
        <ContextMenuItem :class="itemClass" :disabled="!canPlay" @select="async () => actions.addToQueue(await tracks())">
          <span class="flex items-center gap-2"><ListPlus class="size-4 text-dim" />Add to Queue</span>
        </ContextMenuItem>
        <ContextMenuSub>
          <ContextMenuSubTrigger :class="itemClass" :disabled="!canPlay">
            <span class="flex items-center gap-2"><ListMusic class="size-4 text-dim" />Add to Playlist</span>
            <ChevronRight class="size-3.5 text-faint" />
          </ContextMenuSubTrigger>
          <ContextMenuPortal>
            <ContextMenuSubContent :class="contentClass" data-kw-fade :side-offset="6">
              <ContextMenuItem
                v-for="p in playlists.items"
                :key="p.id"
                :class="itemClass"
                @select="async () => actions.addToPlaylist(p.id, await tracks())"
              >
                {{ p.name }} <span class="text-[11px] text-faint">({{ p.track_ids.length }})</span>
              </ContextMenuItem>
              <ContextMenuSeparator v-if="playlists.items.length" class="my-1 h-px bg-line" />
              <ContextMenuItem :class="itemClass" @select="naming = true">
                <span class="flex items-center gap-2"><Plus class="size-4 text-dim" />New playlist…</span>
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuPortal>
        </ContextMenuSub>
        <ContextMenuSeparator class="my-1 h-px bg-line" />
        <ContextMenuItem :class="itemClass" @select="info">
          <span class="flex items-center gap-2"><Info class="size-4 text-dim" />Info</span>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenuPortal>
  </ContextMenuRoot>
  <PromptDialog
    v-model:open="naming"
    title="New playlist"
    label="Playlist name"
    placeholder="Playlist name"
    confirm-label="Create and add"
    @submit="async (name: string) => actions.newPlaylistWith(name, await tracks())"
  />
</template>
