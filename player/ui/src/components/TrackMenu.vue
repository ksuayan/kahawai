<script setup lang="ts">
import { Check, ChevronRight, Disc3, Ellipsis, Info, ListMusic, ListPlus, ListStart, Plus, Radio } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuItemIndicator,
  DropdownMenuPortal,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuRoot,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "reka-ui";
import { useLibraryActions } from "../lib/libraryActions";
import { useJobsStore } from "../stores/jobs";
import { useOverlaysStore } from "../stores/overlays";
import { usePlayerStore } from "../stores/player";
import { usePlaylistsStore } from "../stores/playlists";
import { useQueueStore } from "../stores/queue";
import { useToastsStore } from "../stores/toasts";
import { isPlayable, trackTitle, validFormatsFor, type StreamFormat, type Track } from "../types";
import PromptDialog from "../ui/PromptDialog.vue";
import UiButton from "../ui/UiButton.vue";

/**
 * Reusable "⋯" action menu for a track or a track list (album).
 * Actions: play next, add to queue, add to playlist (neither adds a track
 * that's already there), extract SACD ISO, info.
 */
const props = withDefaults(
  defineProps<{
    /** Single track context. */
    track?: Track | null;
    /** Multi-track context (album). `track` wins when both are given. */
    tracks?: Track[] | null;
    albumId?: number | null;
    /** "menu" (default): one ⋯ button. "buttons": labelled Add to queue / Add to playlist… buttons. */
    layout?: "menu" | "buttons";
  }>(),
  { track: null, tracks: null, albumId: null, layout: "menu" },
);

const queue = useQueueStore();
const playlists = usePlaylistsStore();
const jobs = useJobsStore();
const player = usePlayerStore();
const toasts = useToastsStore();
const actions = useLibraryActions();
const overlays = useOverlaysStore();

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
  await actions.addToQueue(list.value);
}

async function addToPlaylist(id: number): Promise<void> {
  busy.value = true;
  try {
    await actions.addToPlaylist(id, list.value);
  } finally {
    busy.value = false;
  }
}

async function createAndAdd(name: string): Promise<void> {
  busy.value = true;
  try {
    await actions.newPlaylistWith(name, list.value);
  } finally {
    busy.value = false;
  }
}

function showInfo(): void {
  if (props.track) overlays.showInfo({ kind: "track", track: props.track });
}

// --- "Stream as…": a one-track format override, an escape hatch ---------------
// Auto (the default) lets the player negotiate with the output device. Forcing a
// format overrides all of that for this track, and lossy ones cost quality.
const FORMAT_LABELS: Record<StreamFormat, string> = {
  passthrough: "Original file",
  flac: "FLAC",
  opus: "Opus (lossy)",
  mp3: "MP3 (lossy)",
  dop: "DSD over PCM (DoP)",
};

const streamOptions = computed(() => [
  { value: "auto", label: "Auto (recommended)" },
  ...(props.track ? validFormatsFor(props.track) : []).map((f) => ({ value: f, label: FORMAT_LABELS[f] })),
]);

/** The radio value: the forced format, or "auto" when nothing is forced. */
const streamValue = computed(() => player.formatOverride(props.track?.id) ?? "auto");

function chooseStream(v: string): void {
  if (!props.track) return;
  void player.changeTrackFormat(props.track.id, v === "auto" ? null : (v as StreamFormat));
}

function extractIso(): void {
  if (props.track) void jobs.extractIso(props.track);
}

const itemClass =
  "flex cursor-default select-none items-center justify-between gap-3 whitespace-nowrap rounded-md px-2.5 py-2 text-[13px] text-fg outline-none " +
  "data-[disabled]:opacity-40 data-[highlighted]:bg-hover data-[state=open]:bg-hover";
const contentClass =
  "z-[60] max-h-80 min-w-[200px] overflow-y-auto rounded-md border border-line bg-surface p-1 shadow-float";

const menuLabel = computed(() => {
  const t = props.track;
  return t ? `Actions for ${trackTitle(t)}` : "Actions";
});
</script>

<template>
  <span class="inline-block" @dblclick.stop>
    <span v-if="layout === 'buttons'" class="flex flex-wrap items-center gap-2" data-testid="track-actions">
      <UiButton :disabled="list.length === 0 || busy" data-testid="add-to-queue" @click="addToQueue">
        <ListPlus /> Add to queue
      </UiButton>
      <DropdownMenuRoot>
        <DropdownMenuTrigger as-child>
          <UiButton :disabled="list.length === 0 || busy" data-testid="add-to-playlist">
            <ListMusic /> Add to playlist…
          </UiButton>
        </DropdownMenuTrigger>
        <DropdownMenuPortal>
          <DropdownMenuContent align="start" :side-offset="4" :class="contentClass" data-kw-fade>
            <DropdownMenuItem v-for="p in playlists.items" :key="p.id" :class="itemClass" :disabled="busy" @select="addToPlaylist(p.id)">
              {{ p.name }} <span class="text-[11px] text-faint">({{ p.track_ids.length }})</span>
            </DropdownMenuItem>
            <DropdownMenuSeparator v-if="playlists.items.length" class="my-1 h-px bg-line" />
            <DropdownMenuItem :class="itemClass" :disabled="busy" @select="naming = true">
              <span class="flex items-center gap-2"><Plus class="size-4 text-dim" />New playlist…</span>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenuPortal>
      </DropdownMenuRoot>
    </span>
    <DropdownMenuRoot v-else>
      <DropdownMenuTrigger as-child>
        <UiButton variant="icon" :title="menuLabel" :aria-label="menuLabel"><Ellipsis /></UiButton>
      </DropdownMenuTrigger>
      <DropdownMenuPortal>
        <DropdownMenuContent align="end" :side-offset="4" :class="contentClass" data-kw-fade>
          <DropdownMenuItem :class="itemClass" :disabled="list.length === 0 || busy" @select="playNext">
            <span class="flex items-center gap-2"><ListStart class="size-4 text-dim" />Play next</span>
          </DropdownMenuItem>
          <DropdownMenuItem :class="itemClass" :disabled="list.length === 0 || busy" @select="addToQueue">
            <span class="flex items-center gap-2"><ListPlus class="size-4 text-dim" />Add to queue</span>
          </DropdownMenuItem>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger :class="itemClass" :disabled="list.length === 0 || busy">
              <span class="flex items-center gap-2"><ListMusic class="size-4 text-dim" />Add to playlist…</span>
              <ChevronRight class="size-3.5 text-faint" />
            </DropdownMenuSubTrigger>
            <DropdownMenuPortal>
              <DropdownMenuSubContent :class="contentClass" data-kw-fade :side-offset="6">
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
                  <span class="flex items-center gap-2"><Plus class="size-4 text-dim" />New playlist…</span>
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuPortal>
          </DropdownMenuSub>
          <DropdownMenuItem v-if="canExtractIso" :class="itemClass" :disabled="busy" @select="extractIso">
            <span class="flex items-center gap-2"><Disc3 class="size-4 text-dim" />Extract to DSF</span>
          </DropdownMenuItem>
          <DropdownMenuItem v-if="track" :class="itemClass" @select="showInfo">
            <span class="flex items-center gap-2"><Info class="size-4 text-dim" />Info</span>
          </DropdownMenuItem>
          <DropdownMenuSub v-if="track">
            <DropdownMenuSubTrigger :class="itemClass" :disabled="!isPlayable(track)" data-testid="stream-as">
              <span class="flex items-center gap-2">
                <Radio class="size-4 text-dim" />Stream as…
                <span v-if="streamValue !== 'auto'" class="text-[11px] text-accent">{{ FORMAT_LABELS[streamValue as StreamFormat] }}</span>
              </span>
              <ChevronRight class="size-3.5 text-faint" />
            </DropdownMenuSubTrigger>
            <DropdownMenuPortal>
              <DropdownMenuSubContent :class="contentClass" data-kw-fade :side-offset="6">
                <DropdownMenuRadioGroup :model-value="streamValue" @update:model-value="(v) => chooseStream(String(v))">
                  <DropdownMenuRadioItem v-for="o in streamOptions" :key="o.value" :value="o.value" :class="itemClass" :data-testid="`stream-${o.value}`">
                    <span>{{ o.label }}</span>
                    <DropdownMenuItemIndicator><Check class="size-4 text-accent" /></DropdownMenuItemIndicator>
                  </DropdownMenuRadioItem>
                </DropdownMenuRadioGroup>
              </DropdownMenuSubContent>
            </DropdownMenuPortal>
          </DropdownMenuSub>
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
