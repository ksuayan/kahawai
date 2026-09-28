// Direct REST client for the music server. All browse calls go straight
// from the webview to the server via fetch; the Rust core owns playback only.

import type {
  Album,
  Artist,
  ImportPlaylistResult,
  JobInfo,
  JobKind,
  Page,
  Playlist,
  Track,
} from "./types";
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

async function get<T>(path: string): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`);
  if (!res.ok) await fail("GET", path, res);
  return (await res.json()) as T;
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) await fail("POST", path, res);
  return (await res.json()) as T;
}

async function patch<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) await fail("PATCH", path, res);
  return (await res.json()) as T;
}

async function put<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) await fail("PUT", path, res);
  return (await res.json()) as T;
}

async function del(path: string): Promise<void> {
  const res = await fetch(`${baseUrl}${path}`, { method: "DELETE" });
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

export { put };
