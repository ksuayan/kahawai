// Mirrors the shapes returned by src/desktop.rs's Tauri commands
// (see kahawai-server-desktop-ui-spec.md).

export interface ServerConfigShape {
  music_dirs: string[];
  bind: string;
  db_path: string;
  preferred_ladder: string[];
  dsd_story: "pcm" | "native";
  scan_on_startup: boolean;
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
}

/** Client-side status chip derived from a `DirValidation`. */
export type DirStatus = "ok" | "warn" | "err";

export function dirStatus(v: DirValidation): DirStatus {
  if (!v.exists || !v.is_dir || !v.readable) return "err";
  if (v.audio_files === 0) return "warn";
  return "ok";
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

export interface ApplyConfigInput {
  music_dirs: string[];
}

/** Server job (`setup_recent_scans`, mirrors `kahawai_core::Job`). */
export type JobStatus = "queued" | "running" | "done" | "failed";

export interface ScanJob {
  id: string;
  kind: "scan";
  label: string;
  progress: number;
  status: JobStatus;
  message?: string | null;
}

export function isJobActive(j: ScanJob): boolean {
  return j.status === "queued" || j.status === "running";
}
