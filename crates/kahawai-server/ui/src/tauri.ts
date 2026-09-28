// Typed wrappers around the Tauri command bridge (see src/desktop.rs).
// Read-only lookups fail softly under plain `vite dev` (no Tauri webview),
// same idiom as the player's `src/tauri.ts`. Mutating calls that the wizard
// needs to react to (save, start) propagate their `Result<_, String>` so the
// UI can show the error instead of silently doing nothing.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  ApplyConfigInput,
  DirValidation,
  ScanJob,
  ServerConfigShape,
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
 *  restart) and triggers a rescan. Throws the backend's error string. */
export async function setupApplyConfig(input: ApplyConfigInput): Promise<void> {
  await invoke("setup_apply_config", { input });
}

/** Most recent scan jobs (successes and failures), newest first. */
export async function setupRecentScans(): Promise<ScanJob[]> {
  return (await cmd<ScanJob[]>("setup_recent_scans")) ?? [];
}

export async function setupQuit(): Promise<void> {
  await cmd("setup_quit");
}
