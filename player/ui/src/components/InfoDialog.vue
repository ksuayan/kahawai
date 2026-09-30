<script setup lang="ts">
import { Disc3, MicVocal } from "lucide-vue-next";
import { computed, ref, watch } from "vue";
import { useLibraryActions } from "../lib/libraryActions";
import { useLibraryStore } from "../stores/library";
import { useOverlaysStore } from "../stores/overlays";
import {
  formatDuration,
  mqaLabel,
  qualityTitle,
  trackTitle,
  unplayableReason,
  type Album,
  type Track,
} from "../types";
import UiButton from "../ui/UiButton.vue";
import UiDialog from "../ui/UiDialog.vue";
import Artwork from "./Artwork.vue";

/**
 * Info for a track or an album (from the item menus). Beyond what the lists
 * show: the audio format in full, the genre tags, where the files live (what
 * tells duplicate copies apart), the content hash, and the MusicBrainz
 * release when known.
 */
const overlays = useOverlaysStore();
const lib = useLibraryStore();
const actions = useLibraryActions();

const open = computed({
  get: () => overlays.info !== null,
  set: (v: boolean) => {
    if (!v) overlays.info = null;
  },
});

const track = computed<Track | null>(() => (overlays.info?.kind === "track" ? overlays.info.track : null));
/** The album shown, or the track's album. */
const album = computed<Album | null>(() => {
  const info = overlays.info;
  if (!info) return null;
  if (info.kind === "album") return info.album;
  return lib.albums.find((a) => a.id === info.track.album_id) ?? null;
});

// An album's tracks, for its formats, genres, length and folders.
const albumTracks = ref<Track[]>([]);
const loadingTracks = ref(false);
watch(
  () => overlays.info,
  async (info) => {
    albumTracks.value = [];
    if (info?.kind !== "album") return;
    loadingTracks.value = true;
    try {
      albumTracks.value = await actions.albumTracks(info.album);
    } catch {
      // Offline without a cached copy: the rest of the info still shows.
    } finally {
      loadingTracks.value = false;
    }
  },
  { immediate: true },
);

const folderOf = (path: string) => path.slice(0, path.lastIndexOf("/")) || path;
const distinct = (xs: (string | null | undefined)[]) =>
  [...new Set(xs.map((x) => x?.trim()).filter((x): x is string => !!x))];

const cover = computed(() => album.value?.artwork_hash ?? (track.value ? lib.artworkFor(track.value) : null));
const title = computed(() => (track.value ? trackTitle(track.value) : album.value?.title ?? ""));
const artist = computed(() => track.value?.artist ?? album.value?.artist ?? null);

/** Label/value rows; empty values are left out. */
const rows = computed<[string, string][]>(() => {
  const t = track.value;
  const a = album.value;
  const out: [string, string | null | undefined][] = [];
  if (t) {
    out.push(
      ["Album", t.album ?? a?.title],
      ["Album artist", a?.artist && a.artist !== t.artist ? a.artist : null],
      ["Year", t.year ? String(t.year) : a?.year ? String(a.year) : null],
      ["Disc / track", t.track_no ? `${t.disc_no ? `Disc ${t.disc_no} · ` : ""}Track ${t.track_no}` : null],
      ["Length", t.duration_ms ? formatDuration(t.duration_ms) : null],
      ["Genre", t.genre],
      ["Format", qualityTitle(t)],
      ["MQA", t.mqa ? mqaLabel(t) : null],
      ["Playable", t.missing || !t.decodable ? `No: ${unplayableReason(t)}` : null],
      ["File", t.path],
      ["Content hash", t.hash ? `${t.hash.slice(0, 16)}… (BLAKE3)` : "Not hashed yet"],
    );
  } else if (a) {
    const ts = albumTracks.value;
    const discs = new Set(ts.map((x) => x.disc_no ?? 1)).size;
    const length = ts.reduce((s, x) => s + (x.duration_ms ?? 0), 0);
    out.push(
      ["Year", a.year ? String(a.year) : null],
      ["Tracks", ts.length ? `${ts.length}${discs > 1 ? ` on ${discs} discs` : ""}` : loadingTracks.value ? "…" : null],
      ["Length", length ? formatDuration(length) : null],
      ["Genres", distinct(ts.map((x) => x.genre)).join(", ")],
      ["Formats", distinct(ts.map(qualityTitle)).join("\n")],
      ["Folders", distinct(ts.map((x) => folderOf(x.path))).join("\n")],
    );
  }
  if (a) {
    out.push(
      ["MusicBrainz release", a.mbid ?? null],
      [
        "Cover",
        a.artwork_hash
          ? a.artwork_source === "caa"
            ? "From Cover Art Archive"
            : "Embedded in the files"
          : "None",
      ],
    );
  }
  return out.filter((r): r is [string, string] => !!r[1]);
});

/** Values that are paths or IDs: shown in full, selectable, wrapped anywhere. */
const RAW = new Set(["File", "Folders", "Content hash", "MusicBrainz release"]);

function go(where: "artist" | "album"): void {
  const t = track.value;
  const a = album.value;
  open.value = false;
  if (where === "artist") actions.goToArtist(artist.value);
  else actions.goToAlbum(t?.album_id ?? a?.id);
}
</script>

<template>
  <UiDialog v-model:open="open" :title="track ? 'Track info' : 'Album info'" wide>
    <div v-if="overlays.info" class="flex flex-col gap-5 sm:flex-row" data-testid="info-dialog">
      <div class="w-40 shrink-0">
        <Artwork :hash="cover" :size="160" :radius="6" :alt="album?.title ?? title" />
      </div>
      <div class="min-w-0 flex-1">
        <h3 class="heading-2 m-0 mb-1 break-words">{{ title }}</h3>
        <p v-if="artist" class="m-0 mb-4 text-dim">{{ artist }}</p>
        <dl class="m-0 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-[13px]">
          <template v-for="[label, value] in rows" :key="label">
            <dt class="text-dim">{{ label }}</dt>
            <dd
              class="m-0 whitespace-pre-line"
              :class="RAW.has(label) && 'select-text break-all font-mono text-xs'"
              :data-testid="`info-${label}`"
            >
              {{ value }}
            </dd>
          </template>
        </dl>
      </div>
    </div>
    <template #footer>
      <UiButton :disabled="!actions.findArtist(artist)" @click="go('artist')"><MicVocal /> Go to Artist</UiButton>
      <UiButton v-if="track" :disabled="track.album_id == null" @click="go('album')"><Disc3 /> Go to Album</UiButton>
      <UiButton variant="primary" @click="open = false">Close</UiButton>
    </template>
  </UiDialog>
</template>
