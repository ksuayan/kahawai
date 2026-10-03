// Direct REST client for the music server. All browse calls go straight
// from the webview to the server via fetch; the Rust core owns playback only.

import type {
  Album,
  Artist,
  Audiobook,
  AudiobookBookmark,
  AudiobookDetail,
  AudiobookSession,
  AudiobookSettings,
  Genre,
  ImportPlaylistResult,
  JobInfo,
  JobKind,
  Page,
  Playlist,
  PodcastEpisode,
  PodcastEpisodeDetail,
  PodcastFeed,
  PodcastFeedSettings,
  PodcastFolder,
  PodcastImported,
  PodcastSubscribed,
  RadioFacet,
  RadioFavorite,
  RadioPlayInfo,
  RadioQuery,
  RadioStation,
  Track,
} from "./types";
import { parseSort, type SortKey } from "./lib/sorting";
import { cachedArtworkUrl, inTauri } from "./tauri";

let baseUrl = "http://localhost:8080";

export function setBaseUrl(url: string): void {
  baseUrl = url.trim().replace(/\/+$/, "") || "http://localhost:8080";
  reconnectCatalogEvents();
}

export function getBaseUrl(): string {
  return baseUrl;
}

// --- Server events (GET /api/events, SSE) -----------------------------------
// A scan finishing anywhere — this app's own "Scan" button, or the desktop
// Server app's wizard/status screen — pushes a catalog-updated event here;
// without it, a scan triggered elsewhere would only ever be noticed by the
// player that started it (see stores/jobs.ts, which only polls while a job
// it knows about is active). The server also pushes a shutdown notice.
//
// Reconnection is handled manually (not the browser's built-in EventSource
// retry) so the interval is ours to control — currently a fixed 3s, but
// structured so a future Settings pulldown (3/5/10/15s) can change it via
// `setServerEventsRetryMs` without touching the reconnect logic itself.

let eventSource: EventSource | null = null;
let reconnectTimer: number | undefined;
/** True once `open` has fired at least once for the *current* `baseUrl` —
 *  guards against announcing a "disconnect" before ever having connected. */
let everConnected = false;
let isConnected = false;
let retryMs = 3000;

/** For a future Settings screen. Takes effect on the next reconnect
 *  attempt; does not tear down an already-scheduled one. */
export function setServerEventsRetryMs(ms: number): void {
  retryMs = ms;
}

const catalogListeners = new Set<() => void>();
const shuttingDownListeners = new Set<() => void>();
const connectedListeners = new Set<() => void>();
const disconnectedListeners = new Set<() => void>();

function reconnectCatalogEvents(): void {
  eventSource?.close();
  eventSource = null;
  window.clearTimeout(reconnectTimer);
  everConnected = false;
  isConnected = false;
  connectServerEvents();
}

function connectServerEvents(): void {
  if (typeof EventSource === "undefined") return; // defensive; every real target has it
  const source = new EventSource(`${baseUrl}/api/events`);
  eventSource = source;

  source.addEventListener("open", () => {
    if (source !== eventSource) return; // a stale instance from a prior baseUrl
    everConnected = true;
    isConnected = true;
    for (const cb of connectedListeners) cb();
  });
  source.addEventListener("error", () => {
    if (source !== eventSource) return;
    source.close();
    if (everConnected && isConnected) {
      isConnected = false;
      for (const cb of disconnectedListeners) cb();
    }
    reconnectTimer = window.setTimeout(connectServerEvents, retryMs);
  });
  source.addEventListener("catalog-updated", () => {
    if (source !== eventSource) return;
    for (const cb of catalogListeners) cb();
  });
  source.addEventListener("server-shutting-down", () => {
    if (source !== eventSource) return;
    for (const cb of shuttingDownListeners) cb();
  });
}

/** Subscribe to catalog-change notifications. Returns an unsubscribe fn. */
export function onCatalogUpdated(cb: () => void): () => void {
  catalogListeners.add(cb);
  return () => catalogListeners.delete(cb);
}

/** The server sent an explicit "about to exit" notice (graceful shutdown).
 *  Fires before the connection actually drops — `onServerDisconnected`
 *  fires shortly after, same as any other disconnect. */
export function onServerShuttingDown(cb: () => void): () => void {
  shuttingDownListeners.add(cb);
  return () => shuttingDownListeners.delete(cb);
}

/** Fires on every successful (re)connection, including the first one. */
export function onServerConnected(cb: () => void): () => void {
  connectedListeners.add(cb);
  return () => connectedListeners.delete(cb);
}

/** Fires once per actual drop (not once per failed retry attempt) — after a
 *  prior successful connection is lost, whether from a graceful shutdown, a
 *  crash, or a network blip. Reconnection keeps retrying every `retryMs`
 *  regardless; this is purely a notification for the UI. */
export function onServerDisconnected(cb: () => void): () => void {
  disconnectedListeners.add(cb);
  return () => disconnectedListeners.delete(cb);
}

/** Test-only: these listener sets are module-level (there's exactly one
 *  server connection per app instance), so without this, a subscription
 *  left registered by one test (a store's `init()` whose `stop()` the test
 *  never called) would still fire — and push toasts — in every later test. */
export function resetServerEventListenersForTest(): void {
  catalogListeners.clear();
  shuttingDownListeners.clear();
  connectedListeners.clear();
  disconnectedListeners.clear();
}

/**
 * `<img src>` for a cover. In the app it goes through the shell's disk
 * cache; under plain `vite dev` (no shell) it hits the server directly.
 */
export function artworkSrc(hash?: string | null): string {
  if (!hash) return "";
  if (inTauri()) return cachedArtworkUrl(hash);
  return `${baseUrl}/api/artwork/${encodeURIComponent(hash)}`;
}

/**
 * HTTP error carrying the status code and the server's message verbatim.
 * Callers that need special handling (409 "scan already running",
 * the sacd_extract failure message) match on `status` / `message`.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly body: string;

  constructor(method: string, path: string, status: number, body: string) {
    // Prefer the server's own message; it is the honest failure reason.
    super(body || `${method} ${path}: ${status}`);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
  }
}

async function fail(method: string, path: string, res: Response): Promise<never> {
  let body = "";
  try {
    const text = await res.text();
    // The server wraps errors as JSON {"error": ...}; use the message
    // verbatim when present, else the raw text.
    try {
      const j = JSON.parse(text) as { error?: unknown; message?: unknown };
      const msg = j.error ?? j.message;
      body = typeof msg === "string" ? msg : text;
    } catch {
      body = text;
    }
  } catch {
    /* ignore */
  }
  throw new ApiError(method, path, res.status, body.trim());
}

/** Who is listening to audiobooks; "" is the server's default listener. */
let listener = "";
export function setAudiobookListener(name: string): void {
  listener = name.trim();
}

/** Request headers; audiobook calls also say whose place, bookmarks and history they mean. */
function headers(path: string, json = false): Record<string, string> {
  const h: Record<string, string> = json ? { "Content-Type": "application/json" } : {};
  if (listener && path.startsWith("/api/audiobook")) h["X-Kahawai-Listener"] = encodeURIComponent(listener);
  return h;
}

async function get<T>(path: string): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, { headers: headers(path) });
  if (!res.ok) await fail("GET", path, res);
  return (await res.json()) as T;
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: headers(path, true),
    body: JSON.stringify(body),
  });
  if (!res.ok) await fail("POST", path, res);
  return (await res.json()) as T;
}

async function patch<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: "PATCH",
    headers: headers(path, true),
    body: JSON.stringify(body),
  });
  if (!res.ok) await fail("PATCH", path, res);
  return (await res.json()) as T;
}

async function put<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: "PUT",
    headers: headers(path, true),
    body: JSON.stringify(body),
  });
  if (!res.ok) await fail("PUT", path, res);
  return (await res.json()) as T;
}

async function del(path: string): Promise<void> {
  const res = await fetch(`${baseUrl}${path}`, { method: "DELETE", headers: headers(path) });
  if (!res.ok) await fail("DELETE", path, res);
}

function asItems<T>(res: Page<T> | T[]): T[] {
  if (Array.isArray(res)) return res;
  return Array.isArray(res.items) ? res.items : [];
}

/** Fetch every album, walking pages until a short page arrives. */
export async function fetchAllAlbums(perPage = 500): Promise<Album[]> {
  const out: Album[] = [];
  let page = 1;
  for (;;) {
    const res = await get<Page<Album> | Album[]>(
      `/api/albums?page=${page}&per_page=${perPage}`,
    );
    const items = asItems(res);
    out.push(...items);
    if (Array.isArray(res) || items.length < perPage || page > 200) break;
    page += 1;
  }
  return out;
}

export interface AlbumDetailResponse {
  album: Album;
  tracks: Track[];
}

/**
 * GET /api/albums/{id}. The server wraps the record as `{album, tracks}`;
 * a bare album is tolerated so an older/looser server still works.
 */
export async function fetchAlbumDetail(id: number): Promise<AlbumDetailResponse> {
  const raw = await get<AlbumDetailResponse | Album>(`/api/albums/${id}`);
  if ("album" in raw && raw.album) {
    return { album: raw.album, tracks: Array.isArray(raw.tracks) ? raw.tracks : [] };
  }
  return { album: raw as Album, tracks: [] };
}

export async function fetchAlbum(id: number): Promise<Album> {
  return (await fetchAlbumDetail(id)).album;
}

export async function fetchArtists(): Promise<Artist[]> {
  const res = await get<Artist[] | { items: Artist[] }>(`/api/artists`);
  return Array.isArray(res) ? res : asItems(res as Page<Artist>);
}

export interface ArtistDetail {
  artist: Artist;
  albums?: Album[];
}

/**
 * GET /api/artists/{id}. The contract is loose at runtime: if the response
 * carries its albums we use them, otherwise the caller falls back to
 * filtering the album list by artist name.
 */
export async function fetchArtistDetail(id: number): Promise<ArtistDetail> {
  const raw = (await get(`/api/artists/${id}`)) as Record<string, unknown>;
  if (raw && typeof raw === "object") {
    if (raw.artist && typeof raw.artist === "object") {
      return {
        artist: raw.artist as Artist,
        albums: Array.isArray(raw.albums) ? (raw.albums as Album[]) : undefined,
      };
    }
    if (Array.isArray(raw.albums)) {
      return { artist: raw as unknown as Artist, albums: raw.albums as Album[] };
    }
    if (typeof raw.name === "string") {
      return { artist: raw as unknown as Artist };
    }
  }
  throw new Error("unexpected /api/artists/{id} response shape");
}

export async function fetchTrack(id: number): Promise<Track> {
  return get<Track>(`/api/tracks/${id}`);
}

export async function searchTracks(q: string): Promise<Track[]> {
  const res = await get<Track[] | { items: Track[] }>(
    `/api/search?q=${encodeURIComponent(q)}`,
  );
  return Array.isArray(res) ? res : asItems(res as Page<Track>);
}

/** GET /api/genres: canonical genres, most tracks first. An older server
 *  without the endpoint yields an empty list. */
export async function fetchGenres(): Promise<Genre[]> {
  try {
    return await get<Genre[]>(`/api/genres`);
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return [];
    throw e;
  }
}

/** GET /api/genres/{name}/tracks: one page of a genre's present tracks. */
export async function fetchGenreTracks(
  name: string,
  page = 1,
  perPage = 200,
  sort: SortKey = "artist-asc",
): Promise<Page<Track>> {
  const { field, dir } = parseSort(sort);
  return get<Page<Track>>(
    `/api/genres/${encodeURIComponent(name)}/tracks?page=${page}&per_page=${perPage}&sort=${field}&order=${dir}`,
  );
}

export async function fetchPlaylists(): Promise<Playlist[]> {
  const res = await get<Playlist[] | { items: Playlist[] }>(`/api/playlists`);
  return Array.isArray(res) ? res : asItems(res as Page<Playlist>);
}

export async function fetchPlaylist(id: number): Promise<Playlist> {
  return get<Playlist>(`/api/playlists/${id}`);
}

export interface CreatePlaylistBody {
  name: string;
  track_ids?: number[];
  from_queue?: boolean;
  queue_track_ids?: number[];
}

export async function createPlaylist(body: CreatePlaylistBody): Promise<Playlist> {
  return post<Playlist>(`/api/playlists`, body);
}

export async function deletePlaylist(id: number): Promise<void> {
  return del(`/api/playlists/${id}`);
}

export async function renamePlaylist(id: number, name: string): Promise<Playlist> {
  return patch<Playlist>(`/api/playlists/${id}`, { name });
}

export interface SetPlaylistTracksBody {
  track_ids?: number[];
  album_ids?: number[];
  /** "append" (default) or "replace". */
  mode?: "append" | "replace";
}

export async function setPlaylistTracks(id: number, body: SetPlaylistTracksBody): Promise<Playlist> {
  return put<Playlist>(`/api/playlists/${id}/tracks`, body);
}

/**
 * Upload an .m3u/.m3u8 file (multipart `file` field, optional `name`).
 * Returns the matched count and the unmatched entry list for the dialog.
 */
export async function importPlaylistFile(
  file: File,
  name?: string,
): Promise<ImportPlaylistResult> {
  const form = new FormData();
  form.append("file", file, file.name);
  if (name) form.append("name", name);
  const res = await fetch(`${baseUrl}/api/playlists/import`, {
    method: "POST",
    body: form,
  });
  if (!res.ok) await fail("POST", "/api/playlists/import", res);
  return (await res.json()) as ImportPlaylistResult;
}

// --- jobs (C3) ---------------------------------------------------------------

export async function fetchJobs(): Promise<JobInfo[]> {
  return get<JobInfo[]>(`/api/jobs`);
}

export interface CreateJobBody {
  kind: JobKind;
  label?: string;
  path?: string;
}

/** `POST /api/jobs` → 202 with the created job. */
export async function createJob(body: CreateJobBody): Promise<JobInfo> {
  return post<JobInfo>(`/api/jobs`, body);
}

/**
 * `POST /api/scan` → 202 with the scan job. A 409 means a scan is already
 * running — the caller surfaces that instead of treating it as an error.
 */
export async function triggerScan(): Promise<JobInfo> {
  try {
    return await post<JobInfo>(`/api/scan`, {});
  } catch (e) {
    if (e instanceof ApiError && e.status === 409) {
      // Normalize: "scan already running" is a state, not a failure.
      throw new ApiError("POST", "/api/scan", 409, "Scan already running");
    }
    throw e;
  }
}

export async function checkHealth(): Promise<boolean> {
  try {
    const res = await get<{ status?: string }>(`/api/health`);
    return res.status === "ok";
  } catch {
    return false;
  }
}

/** `GET /api/identity` (mirrors kahawai_core::ServerIdentity). */
export interface ServerIdentity {
  service: string;
  name: string;
  version: string;
  api_version: number;
  build: { commit: string; dirty: boolean; built_at: string; profile: string; target: string };
  catalog_id: string;
  started_at: number;
  /** Where the server's source code is (its AGPL source offer); empty from
   *  older servers. */
  source_url?: string;
}

/** Every Kahawai server's `service`. */
export const KAHAWAI_SERVICE = "kahawai-server";

/** What answers at the server URL. */
export type ServerCheck =
  | { kind: "kahawai"; identity: ServerIdentity }
  /** A Kahawai server from before /api/identity (it answers /api/health). */
  | { kind: "older" }
  /** Something answers, but it isn't a Kahawai server. */
  | { kind: "other" }
  | { kind: "offline" };

/** Is the server URL a running Kahawai server, and which one? */
export async function checkServer(): Promise<ServerCheck> {
  try {
    const id = await get<Partial<ServerIdentity>>(`/api/identity`);
    return id?.service === KAHAWAI_SERVICE ? { kind: "kahawai", identity: id as ServerIdentity } : { kind: "other" };
  } catch (e) {
    if (!(e instanceof ApiError)) return { kind: "offline" }; // nothing answered
  }
  // It answered, but not /api/identity: an older Kahawai server, or not one.
  return (await checkHealth()) ? { kind: "older" } : { kind: "other" };
}

/** "Kahawai Server 0.1.0 · build cd9b827 (release, aarch64-apple-darwin)". */
export function describeServer(id: ServerIdentity): string {
  const dirty = id.build.dirty ? "+changes" : "";
  return `${id.name} ${id.version} · build ${id.build.commit}${dirty} (${id.build.profile}, ${id.build.target})`;
}

export { put };

// --- Audiobooks ---------------------------------------------------------------

export interface AudiobookQuery {
  /** "continue": books you have started, unfinished first, newest first. */
  shelf?: "continue";
  q?: string;
  author?: string;
  series?: string;
  finished?: boolean;
}

export async function fetchAudiobooks(query: AudiobookQuery = {}): Promise<Audiobook[]> {
  const p = new URLSearchParams();
  if (query.shelf) p.set("shelf", query.shelf);
  if (query.q?.trim()) p.set("q", query.q.trim());
  if (query.author) p.set("author", query.author);
  if (query.series) p.set("series", query.series);
  if (query.finished !== undefined) p.set("finished", String(query.finished));
  const qs = p.toString();
  return get<Audiobook[]>(`/api/audiobooks${qs ? `?${qs}` : ""}`);
}

export async function fetchAudiobook(id: number): Promise<AudiobookDetail> {
  return get<AudiobookDetail>(`/api/audiobooks/${id}`);
}

export async function saveAudiobookPosition(
  id: number,
  bookOffsetMs: number,
): Promise<{ book_offset_ms: number; updated_at: number; finished: boolean }> {
  return put(`/api/audiobooks/${id}/position`, { book_offset_ms: Math.max(0, Math.round(bookOffsetMs)) });
}

export async function markAudiobookFinished(id: number, finished = true): Promise<Audiobook> {
  return post<Audiobook>(`/api/audiobooks/${id}/finished`, { finished });
}

/** Take a book off the Continue listening shelf; its place is kept. */
export async function dismissAudiobookFromShelf(id: number): Promise<void> {
  await del(`/api/audiobooks/${id}/continue`);
}

export async function fetchAudiobookHistory(id: number): Promise<AudiobookSession[]> {
  return get<AudiobookSession[]>(`/api/audiobooks/${id}/history`);
}

export async function addAudiobookBookmark(
  id: number,
  bookOffsetMs: number,
  name?: string,
  note?: string,
): Promise<AudiobookBookmark> {
  return post<AudiobookBookmark>(`/api/audiobooks/${id}/bookmarks`, {
    book_offset_ms: Math.max(0, Math.round(bookOffsetMs)),
    name,
    note,
  });
}

export async function editAudiobookBookmark(
  id: number,
  bookmarkId: number,
  edit: { name?: string; note?: string },
): Promise<AudiobookBookmark> {
  return patch<AudiobookBookmark>(`/api/audiobooks/${id}/bookmarks/${bookmarkId}`, edit);
}

export async function deleteAudiobookBookmark(id: number, bookmarkId: number): Promise<void> {
  return del(`/api/audiobooks/${id}/bookmarks/${bookmarkId}`);
}

export async function saveAudiobookSettings(id: number, s: Partial<AudiobookSettings>): Promise<AudiobookSettings> {
  return put<AudiobookSettings>(`/api/audiobooks/${id}/settings`, s);
}

export interface AudiobookMetaEdit {
  title?: string;
  author?: string;
  narrator?: string;
  series?: string;
  series_index?: number;
  year?: number;
}

export async function editAudiobook(id: number, edit: AudiobookMetaEdit): Promise<Audiobook> {
  return patch<Audiobook>(`/api/audiobooks/${id}`, edit);
}

export interface AudiobookListener {
  id: number;
  name: string;
  books_started: number;
}

export async function fetchAudiobookListeners(): Promise<AudiobookListener[]> {
  return get<AudiobookListener[]>("/api/audiobook-listeners");
}

export async function addAudiobookListener(name: string): Promise<AudiobookListener> {
  return post<AudiobookListener>("/api/audiobook-listeners", { name });
}

export async function deleteAudiobookListener(id: number): Promise<void> {
  return del(`/api/audiobook-listeners/${id}`);
}

// --- internet radio ------------------------------------------------------------

export async function searchRadio(q: RadioQuery): Promise<RadioStation[]> {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(q)) if (v !== undefined && v !== "") p.set(k, String(v));
  const qs = p.toString();
  return get<RadioStation[]>(`/api/radio/search${qs ? `?${qs}` : ""}`);
}

export async function fetchRadioFacets(kind: "tags" | "countries" | "languages"): Promise<RadioFacet[]> {
  return get<RadioFacet[]>(`/api/radio/facets/${kind}`);
}

export async function fetchRadioFavorites(): Promise<RadioFavorite[]> {
  return get<RadioFavorite[]>("/api/radio/favorites");
}

/** Save a directory station, or add one by address (`url` only). */
export async function addRadioFavorite(
  body: Partial<Omit<RadioFavorite, "id" | "manual" | "sort_order" | "added_at">> & { url: string },
): Promise<RadioFavorite> {
  return post<RadioFavorite>("/api/radio/favorites", body);
}

export async function renameRadioFavorite(id: number, name: string): Promise<RadioFavorite> {
  return patch<RadioFavorite>(`/api/radio/favorites/${id}`, { name });
}

export async function deleteRadioFavorite(id: number): Promise<void> {
  return del(`/api/radio/favorites/${id}`);
}

export async function reorderRadioFavorites(ids: number[]): Promise<RadioFavorite[]> {
  return put<RadioFavorite[]>("/api/radio/favorites/order", { ids });
}

/** The address to connect to (the directory's fresh one when it is on). */
export async function playRadioFavorite(id: number): Promise<RadioPlayInfo> {
  return post<RadioPlayInfo>(`/api/radio/favorites/${id}/play`, {});
}

export async function logRadioHeard(stationName: string, streamTitle: string): Promise<void> {
  await post("/api/radio/history", { station_name: stationName, stream_title: streamTitle }).catch(() => undefined);
}

/** Look up the missing author, year and cover of one book (or every book without them) online. */
export async function enrichAudiobooks(bookId?: number): Promise<JobInfo> {
  return post<JobInfo>("/api/audiobooks/enrich", bookId === undefined ? {} : { book_id: bookId });
}

// --- podcasts --------------------------------------------------------------------

export async function fetchPodcastFeeds(): Promise<PodcastFeed[]> {
  return get<PodcastFeed[]>("/api/podcasts/feeds");
}

/** Subscribe by the feed's address; the server reads it first and says why not. */
export async function subscribePodcast(url: string): Promise<PodcastSubscribed> {
  return post<PodcastSubscribed>("/api/podcasts/feeds", { url: url.trim() });
}

export async function unsubscribePodcast(feedId: number): Promise<void> {
  await del(`/api/podcasts/feeds/${feedId}`);
}

/** Import another app's OPML export (the file's text is the body). */
export async function importPodcastOpml(text: string): Promise<PodcastImported> {
  const path = "/api/podcasts/feeds/import-opml";
  const res = await fetch(`${baseUrl}${path}`, { method: "POST", headers: { "Content-Type": "text/x-opml" }, body: text });
  if (!res.ok) await fail("POST", path, res);
  return (await res.json()) as PodcastImported;
}

/** Where the subscriptions download as an OPML file (the server sends it as a file to save). */
export function podcastOpmlExportUrl(): string {
  return `${baseUrl}/api/podcasts/feeds/export-opml`;
}

export async function fetchPodcastEpisodes(feedId: number, unplayedOnly = false): Promise<PodcastEpisode[]> {
  return get<PodcastEpisode[]>(`/api/podcasts/feeds/${feedId}/episodes${unplayedOnly ? "?unplayed=1" : ""}`);
}

export async function fetchPodcastEpisode(id: number): Promise<PodcastEpisodeDetail> {
  return get<PodcastEpisodeDetail>(`/api/podcasts/episodes/${id}`);
}

export async function fetchPodcastEpisodeHistory(id: number): Promise<AudiobookSession[]> {
  return get<AudiobookSession[]>(`/api/podcasts/episodes/${id}/history`);
}

export async function fetchPodcastsInProgress(): Promise<PodcastEpisode[]> {
  return get<PodcastEpisode[]>("/api/podcasts/in-progress");
}

export async function fetchPodcastDownloads(): Promise<PodcastEpisode[]> {
  return get<PodcastEpisode[]>("/api/podcasts/downloads");
}

export async function fetchPodcastFolder(): Promise<PodcastFolder> {
  return get<PodcastFolder>("/api/podcasts/folder");
}

/** Read one feed again (or all of them); the answer says what changed or why not. */
export async function refreshPodcasts(feedId?: number): Promise<{ feed_id: number; episodes_added: number; error: string | null }[]> {
  return post("/api/podcasts/refresh", feedId == null ? {} : { feed_id: feedId });
}

export async function savePodcastFeedSettings(feedId: number, s: PodcastFeedSettings): Promise<PodcastFeed> {
  return put<PodcastFeed>(`/api/podcasts/feeds/${feedId}/settings`, s);
}

export async function savePodcastPosition(id: number, offsetMs: number): Promise<{ offset_ms: number; updated_at: number; played: boolean }> {
  return put(`/api/podcasts/episodes/${id}/position`, { offset_ms: Math.max(0, Math.round(offsetMs)) });
}

export async function markPodcastEpisodePlayed(id: number, played: boolean): Promise<void> {
  const path = `/api/podcasts/episodes/${id}/played`;
  const res = await fetch(`${baseUrl}${path}`, { method: "POST", headers: headers(path, true), body: JSON.stringify({ played }) });
  if (!res.ok) await fail("POST", path, res);
}

/** Queue the download on the server (a job). */
export async function downloadPodcastEpisode(id: number): Promise<JobInfo> {
  return post<JobInfo>(`/api/podcasts/episodes/${id}/download`, {});
}

/** Cancel a download, or delete the downloaded file (the episode and its place stay). */
export async function deletePodcastDownload(id: number): Promise<void> {
  await del(`/api/podcasts/episodes/${id}/download`);
}
