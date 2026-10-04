// Typed wrappers around the Tauri command bridge (see src/desktop.rs).
// Read-only lookups fail softly under plain `vite dev` (no Tauri webview),
// same idiom as the player's `src/tauri.ts`. Mutating calls that the wizard
// needs to react to (save, start) propagate their `Result<_, String>` so the
// UI can show the error instead of silently doing nothing.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AudiobookRoot,
  ApplyAudiobooksInput,
  ApplyConfigInput,
  DirValidation,
  EnrichAction,
  EnrichmentStatus,
  LiveScanStats,
  PodcastSettings,
  ScanJob,
  ServerConfigShape,
  ServerIdentity,
  ServerStatus,
  SetupInput,
  SetupState,
} from "./types";

async function cmd<T>(name: string, args?: Record<string, unknown>): Promise<T | undefined> {
  try {
    return await invoke<T>(name, args);
  } catch (err) {
    console.warn(`[tauri] command '${name}' failed:`, err);
    return undefined;
  }
}

/** True inside the Tauri webview (false under plain `vite dev`). */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function setupGetState(): Promise<SetupState | undefined> {
  return cmd<SetupState>("setup_get_state");
}

/** Directory picker. `undefined` on cancel or outside Tauri. */
export async function setupPickDirectory(): Promise<string | undefined> {
  if (!inTauri()) return undefined;
  try {
    const picked = await open({ directory: true, multiple: false });
    return typeof picked === "string" ? picked : undefined;
  } catch (err) {
    console.warn("[tauri] directory picker failed:", err);
    return undefined;
  }
}

export async function setupValidateDir(path: string): Promise<DirValidation | undefined> {
  return cmd<DirValidation>("setup_validate_dir", { path });
}

/** Throws the backend's error string on failure — the wizard shows it inline. */
export async function setupSaveConfig(input: SetupInput): Promise<void> {
  await invoke("setup_save_config", { input });
}

export async function setupStartServer(): Promise<ServerStatus> {
  return invoke<ServerStatus>("setup_start_server");
}

export async function setupServerStatus(): Promise<ServerStatus | undefined> {
  return cmd<ServerStatus>("setup_server_status");
}

export async function setupRevealConfig(): Promise<void> {
  await cmd("setup_reveal_config");
}

/** The live config of the running server — `undefined` if none is running. */
export async function setupGetRunningConfig(): Promise<ServerConfigShape | undefined> {
  return cmd<ServerConfigShape>("setup_get_running_config");
}

/** Add/remove music folders on a running server: applies immediately (no
 *  restart) and triggers a rescan. `add`/`remove` are deltas, not a full
 *  replacement list — a folder is dropped only if named in `remove`.
 *  Throws the backend's error string. */
export async function setupApplyConfig(input: ApplyConfigInput): Promise<void> {
  await invoke("setup_apply_config", { input });
}

/** Most recent scan jobs (successes and failures), newest first. */
export async function setupRecentScans(): Promise<ScanJob[]> {
  return (await cmd<ScanJob[]>("setup_recent_scans")) ?? [];
}

/** The content-hashing job while it is queued or running, else null. */
export async function setupActiveHashJob(): Promise<ScanJob | null> {
  return (await cmd<ScanJob | null>("setup_active_hash_job")) ?? null;
}

/** The audiobook details lookup while it is queued or running, else null. */
export async function setupActiveBookLookup(): Promise<ScanJob | null> {
  return (await cmd<ScanJob | null>("setup_active_book_lookup")) ?? null;
}

/** Live catalog counts, for the Status tab's tally while a scan runs. */
/** Rescan the music folders (audiobook folders ride along). Throws when a scan is already running. */
export async function setupRescanLibrary(): Promise<void> {
  await invoke("setup_rescan_library");
}

/** Rescan the audiobook folders only. Throws when a scan is already running. */
export async function setupRescanAudiobooks(): Promise<void> {
  await invoke("setup_rescan_audiobooks");
}

export async function setupLiveScanStats(): Promise<LiveScanStats> {
  return (
    (await cmd<LiveScanStats>("setup_live_scan_stats")) ?? { albums: 0, artists: 0, tracks: 0 }
  );
}

/** Settings → Album info. `undefined` when no server is running. */
export async function setupEnrichmentStatus(): Promise<EnrichmentStatus | undefined> {
  return cmd<EnrichmentStatus>("setup_enrichment_status");
}

/** Turn online lookup on/off and set its threshold (live and saved).
 *  Throws the backend's error string. */
export async function setupSetEnrichment(enabled: boolean, minConfidence: number): Promise<void> {
  await invoke("setup_set_enrichment", { enabled, minConfidence });
}

/** Settings → Online sources. `undefined` when no server is running. */
export async function setupOnlineSources(): Promise<boolean | undefined> {
  return cmd<boolean>("setup_online_sources");
}

/** Turn the online station and podcast directories on/off (live and saved).
 *  Throws the backend's error string. */
export async function setupSetOnlineSources(enabled: boolean): Promise<void> {
  await invoke("setup_set_online_sources", { enabled });
}

/** Settings → Podcasts. `undefined` when no server is running. */
export async function setupPodcastSettings(): Promise<PodcastSettings | undefined> {
  return cmd<PodcastSettings>("setup_podcast_settings");
}

/** Set the download folder (`null` = the default) and how often feeds are
 *  checked (hours, 0 = never). Throws the backend's reason, e.g. a folder that
 *  cannot be written to. */
export async function setupSetPodcastSettings(dir: string | null, refreshHours: number): Promise<void> {
  await invoke("setup_set_podcast_settings", { dir, refreshHours });
}

/** Start, pause, resume or cancel a lookup. Throws the backend's error string. */
export async function setupEnrichmentAction(action: EnrichAction, jobId?: string): Promise<void> {
  await invoke("setup_enrichment_action", { action, jobId: jobId ?? null });
}

/** Stops the server process without quitting the app. */
export async function setupStopServer(): Promise<void> {
  await cmd("setup_stop_server");
}

/** Stops then starts again from the on-disk config. Throws on a bind failure. */
export async function setupRestartServer(): Promise<ServerStatus> {
  return invoke<ServerStatus>("setup_restart_server");
}

export async function setupQuit(): Promise<void> {
  await cmd("setup_quit");
}

/** The UI is up: the shell swaps its splash window for this one. */
export async function setupAppReady(): Promise<void> {
  if (inTauri()) await cmd("setup_app_ready");
}

/** This app's running server's identity; `undefined` when not running. */
export async function setupServerIdentity(): Promise<ServerIdentity | undefined> {
  return (await cmd<ServerIdentity | null>("setup_server_identity")) ?? undefined;
}

/** Stop the other Kahawai Server holding the port, then start this one.
 *  Throws the backend's error string. */
export async function setupStopOtherServer(): Promise<ServerStatus> {
  return invoke<ServerStatus>("setup_stop_other_server");
}

/** The native app menu's custom items (today "app.about") arrive as their id. */
export async function onMenuAction(cb: (id: string) => void): Promise<UnlistenFn | null> {
  if (!inTauri()) return null;
  try {
    return await listen<string>("menu-action", (event) => cb(event.payload));
  } catch (err) {
    console.error("[tauri] could not subscribe to menu-action:", err);
    return null;
  }
}

/** Show the log folder (~/Library/Logs/Kahawai Server) in Finder. */
export async function setupRevealLogs(): Promise<void> {
  await cmd("setup_reveal_logs");
}

/** The running server's audiobook folders (empty when none is running). */
export async function setupAudiobookFolders(): Promise<AudiobookRoot[]> {
  return (await cmd<AudiobookRoot[]>("setup_audiobook_folders")) ?? [];
}

/** Apply the audiobook folder edits (forget `remove`, add `add`) to the running server,
 *  remember them in the config file, and scan. Throws the backend's reason on a problem. */
export async function setupApplyAudiobooks(input: ApplyAudiobooksInput): Promise<void> {
  await invoke("setup_apply_audiobooks", { input });
}

/** Like `setupValidateDir`, but counts the audiobooks under the folder. */
export async function setupValidateAudiobookDir(path: string): Promise<DirValidation | undefined> {
  return cmd<DirValidation>("setup_validate_audiobook_dir", { path });
}
