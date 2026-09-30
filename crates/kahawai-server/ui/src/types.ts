// Mirrors the shapes returned by src/desktop.rs's Tauri commands
// (see kahawai-server-desktop-ui-spec.md).

export interface ServerConfigShape {
  music_dirs: string[];
  bind: string;
  db_path: string;
  preferred_ladder: string[];
  dsd_story: "pcm" | "native";
  scan_on_startup: boolean;
  enrichment_enabled?: boolean;
  enrichment_min_confidence?: number;
}

export interface SetupState {
  config_path: string;
  config_exists: boolean;
  config?: ServerConfigShape;
}

export interface DirValidation {
  exists: boolean;
  is_dir: boolean;
  readable: boolean;
  writable: boolean;
  audio_files: number;
  /** True if the quick preview walk hit its file cap before finishing —
   *  `audio_files` is then a lower bound, not the true count. The actual
   *  scan is never capped. */
  truncated: boolean;
}

/** Client-side status chip derived from a `DirValidation`. */
export type DirStatus = "ok" | "warn" | "err";

export function dirStatus(v: DirValidation): DirStatus {
  if (!v.exists || !v.is_dir || !v.readable) return "err";
  if (v.audio_files === 0) return "warn";
  return "ok";
}

/** Chip copy shown next to a folder row. Shared by the wizard and Settings
 *  tab so the truncated-count honesty fix only has to live in one place. */
export function dirChipText(v: DirValidation): string {
  const status = dirStatus(v);
  if (status === "err") return "not accessible";
  if (status === "warn") return "no audio files found";
  const suffix = v.truncated ? "+" : "";
  const plural = v.audio_files === 1 && !v.truncated ? "" : "s";
  // Explicit locale: this must render identically regardless of the host's
  // default locale (deterministic tests, consistent UI for every user).
  return `${v.audio_files.toLocaleString("en-US")}${suffix} audio file${plural}`;
}

export function dirChipClass(v: DirValidation): string {
  const status = dirStatus(v);
  if (status === "err") return "text-danger-fg";
  if (status === "warn") return "text-warn-fg";
  return "text-ok";
}

export interface MusicDirEntry {
  path: string;
  validation?: DirValidation;
  /** Set while `setup_validate_dir` is in flight for this row. */
  validating: boolean;
}

export interface SetupInput {
  music_dirs: string[];
  db_dir: string;
  bind: string;
}

export interface ServerStatus {
  running: boolean;
  bind: string;
}

/** Deltas, not a full replacement list — see `setup_apply_config`. */
export interface ApplyConfigInput {
  add: string[];
  remove: string[];
}

export interface LiveScanStats {
  albums: number;
  artists: number;
  tracks: number;
  last_album?: string | null;
  last_album_artist?: string | null;
}

/** Server job (`setup_recent_scans`, mirrors `kahawai_core::Job`).
 *  `paused` and `cancelled` only happen to album info lookups. */
export type JobStatus = "queued" | "running" | "done" | "failed" | "paused" | "cancelled";

export interface ScanJob {
  id: string;
  kind: "scan";
  label: string;
  progress: number;
  status: JobStatus;
  message?: string | null;
}

export function isJobActive(j: { status: JobStatus }): boolean {
  return j.status === "queued" || j.status === "running";
}

/** Online album info lookup job (`enrich_metadata`). */
export interface EnrichJob {
  id: string;
  kind: "enrich_metadata";
  label: string;
  progress: number;
  status: JobStatus;
  message?: string | null;
}

/** `setup_enrichment_status`: Settings → Album info. */
export interface EnrichmentStatus {
  enabled: boolean;
  min_confidence: number;
  coverage: {
    total_albums: number;
    with_embedded_mbid: number;
    matched_online: number;
    no_match: number;
    pending_lookup: number;
  };
  job?: EnrichJob | null;
}

export type EnrichAction = "start" | "pause" | "resume" | "cancel";

/** How strict a MusicBrainz match must be before it's accepted. */
export const CONFIDENCE_LEVELS: { value: number; label: string }[] = [
  { value: 0.8, label: "Relaxed (80%)" },
  { value: 0.9, label: "Balanced (90%)" },
  { value: 0.95, label: "Strict (95%)" },
];
