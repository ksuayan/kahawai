import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import {
  addRadioFavorite,
  deleteRadioFavorite,
  fetchRadioFacets,
  fetchRadioFavorites,
  logRadioHeard,
  playRadioFavorite,
  renameRadioFavorite,
  reorderRadioFavorites,
  searchRadio,
} from "../api";
import { queuePlay } from "../tauri";
import type { RadioFacet, RadioFavorite, RadioOrder, RadioStation, Track, TrackFormat } from "../types";
import { useAudiobooksStore } from "./audiobooks";
import { usePodcastsStore } from "./podcasts";
import { usePlayerStore } from "./player";

/** A station is a queue item with a negative id; its `path` is the stream address. */
export const isStationTrack = (t: Track | null | undefined): boolean => !!t && t.id < 0;

/** The decoder to expect, from the directory's codec word. */
export function formatOfCodec(codec: string | null | undefined): TrackFormat {
  const c = (codec ?? "").toLowerCase();
  if (c.includes("aac")) return "aac";
  if (c.includes("opus")) return "opus";
  if (c.includes("ogg") || c.includes("vorbis")) return "ogg_vorbis";
  if (c.includes("flac")) return "flac";
  return "mp3";
}

/** What the server says when the online directory is switched off. */
const DIRECTORY_OFF = /online station directory is off/i;

interface Query {
  q: string;
  tag: string;
  country: string;
  language: string;
  order: RadioOrder;
}

/**
 * Internet radio: favorites, directory browsing and playing a station. A
 * station plays through the same engine as music, as a queue item whose id is
 * negative; the engine reports its song title and connection state.
 */
export const useRadioStore = defineStore("radio", () => {
  const player = usePlayerStore();
  const books = useAudiobooksStore();

  const favorites = ref<RadioFavorite[]>([]);
  const favoritesLoaded = ref(false);
  const results = ref<RadioStation[]>([]);
  const searched = ref(false);
  const searching = ref(false);
  const error = ref<string | null>(null);
  /** The server's online directory is off (Server settings → Online sources). */
  const directoryOff = ref(false);
  const facets = ref<{ tags: RadioFacet[]; countries: RadioFacet[]; languages: RadioFacet[] }>({
    tags: [],
    countries: [],
    languages: [],
  });
  const query = ref<Query>({ q: "", tag: "", country: "", language: "", order: "clickcount" });
  /** Song titles heard on the current station, newest first. */
  const heard = ref<string[]>([]);
  let nextAdHoc = -1_000_000_000;

  const isPlaying = computed(() => isStationTrack(player.currentTrack));
  const now = computed(() => (isPlaying.value ? (player.raw?.radio ?? null) : null));
  const stationName = computed(() => (isPlaying.value ? (player.currentTrack?.title ?? "") : ""));
  const stationFavorite = computed(() => {
    const id = player.currentTrack?.id;
    return id !== undefined && id < 0 ? (favorites.value.find((f) => f.id === -id) ?? null) : null;
  });

  async function loadFavorites(): Promise<void> {
    try {
      favorites.value = await fetchRadioFavorites();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      favoritesLoaded.value = true;
    }
  }

  function describe(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  async function search(): Promise<void> {
    searching.value = true;
    error.value = null;
    directoryOff.value = false;
    try {
      const q = query.value;
      results.value = await searchRadio({ q: q.q, tag: q.tag, country: q.country, language: q.language, order: q.order, limit: 100 });
      searched.value = true;
    } catch (e) {
      const msg = describe(e);
      if (DIRECTORY_OFF.test(msg)) directoryOff.value = true;
      else error.value = msg;
      results.value = [];
    } finally {
      searching.value = false;
    }
  }

  async function loadFacets(): Promise<void> {
    if (facets.value.tags.length || directoryOff.value) return;
    try {
      const [tags, countries, languages] = await Promise.all([
        fetchRadioFacets("tags"),
        fetchRadioFacets("countries"),
        fetchRadioFacets("languages"),
      ]);
      facets.value = { tags, countries, languages };
    } catch (e) {
      if (DIRECTORY_OFF.test(describe(e))) directoryOff.value = true;
    }
  }

  const favoriteOf = (s: RadioStation): RadioFavorite | undefined =>
    favorites.value.find((f) => (s.station_uuid ? f.station_uuid === s.station_uuid : f.url === s.url));

  async function addFavorite(s: RadioStation): Promise<void> {
    error.value = null;
    try {
      await addRadioFavorite({
        station_uuid: s.station_uuid,
        name: s.name,
        url: s.url,
        url_resolved: s.url_resolved,
        homepage: s.homepage,
        favicon: s.favicon,
        tags: s.tags,
        country: s.country,
        language: s.language,
        bitrate: s.bitrate,
        codec: s.codec,
      });
      await loadFavorites();
    } catch (e) {
      error.value = describe(e);
    }
  }

  /** Add a station by its address; the server checks that it is a stream. Throws its reason. */
  async function addByAddress(url: string): Promise<void> {
    await addRadioFavorite({ url: url.trim() });
    await loadFavorites();
  }

  async function removeFavorite(id: number): Promise<void> {
    await deleteRadioFavorite(id);
    await loadFavorites();
  }

  async function rename(id: number, name: string): Promise<void> {
    await renameRadioFavorite(id, name);
    await loadFavorites();
  }

  async function move(id: number, by: -1 | 1): Promise<void> {
    const ids = favorites.value.map((f) => f.id);
    const i = ids.indexOf(id);
    const j = i + by;
    if (i < 0 || j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    favorites.value = await reorderRadioFavorites(ids);
  }

  /** Put whatever else is playing down (a book keeps its place), then play a station. */
  async function start(name: string, url: string, codec: string | null, bitrate: number | null, id: number): Promise<void> {
    if (books.active) {
      await player.pause();
      await books.reportNow();
      await books.leave(false);
    }
    const podcasts = usePodcastsStore();
    if (podcasts.active) {
      await player.pause();
      await podcasts.reportNow();
      await podcasts.leave(false);
    }
    heard.value = [];
    const track: Track = {
      id,
      path: url,
      format: formatOfCodec(codec),
      bitrate,
      title: name,
      artist: "Live radio",
      album: null,
      missing: false,
      decodable: true,
      duration_ms: null,
    };
    await queuePlay([track], 0);
  }

  async function play(f: RadioFavorite): Promise<void> {
    error.value = null;
    try {
      const info = await playRadioFavorite(f.id);
      await start(info.name, info.url, info.codec ?? f.codec, info.bitrate ?? f.bitrate, -f.id);
    } catch (e) {
      error.value = describe(e);
    }
  }

  /** Play a directory station without saving it. */
  async function playStation(s: RadioStation): Promise<void> {
    const saved = favoriteOf(s);
    if (saved) return play(saved);
    error.value = null;
    nextAdHoc -= 1;
    await start(s.name, s.url_resolved ?? s.url, s.codec, s.bitrate, nextAdHoc);
  }

  // The station says a new song is on: show it in the list and keep a record on the server.
  watch(
    () => now.value?.title ?? null,
    (title) => {
      if (!title) return;
      if (heard.value[0] !== title) heard.value = [title, ...heard.value].slice(0, 50);
      if (stationName.value) void logRadioHeard(stationName.value, title);
    },
  );

  return {
    favorites,
    favoritesLoaded,
    results,
    searched,
    searching,
    error,
    directoryOff,
    facets,
    query,
    heard,
    isPlaying,
    now,
    stationName,
    stationFavorite,
    loadFavorites,
    search,
    loadFacets,
    favoriteOf,
    addFavorite,
    addByAddress,
    removeFavorite,
    rename,
    move,
    play,
    playStation,
  };
});
