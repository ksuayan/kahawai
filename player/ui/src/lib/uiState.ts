import { getUiState, inTauri, setUiState } from "../tauri";

/**
 * The UI's saved preferences, keyed like localStorage (string values).
 *
 * In the app they live in the shell's `ui-state.json`, loaded once before
 * the UI mounts ({@link loadUiState}) and written through on every change.
 * localStorage is kept as a mirror: it's the store outside the app (plain
 * `vite dev` in a browser, tests), and where values saved by earlier
 * versions are found. Those move into the file the first time they're read.
 */
let file: Map<string, string> | null = null;

/** Read the shell's saved state. Call once, before any store is created. */
export async function loadUiState(): Promise<void> {
  if (!inTauri()) return;
  const saved = await getUiState();
  if (!saved) return; // the command failed: localStorage alone, as before
  file = new Map(
    Object.entries(saved).filter((e): e is [string, string] => typeof e[1] === "string"),
  );
}

function localGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function localSet(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Storage unavailable: the file (in the app) still has it.
  }
}

export function uiGet(key: string): string | null {
  if (file?.has(key)) return file.get(key)!;
  const local = localGet(key);
  if (local !== null && file) {
    // Saved by an earlier version (or another origin's run): move it over.
    file.set(key, local);
    void setUiState(key, local);
  }
  return local;
}

/** Save `value` for `key` (`null` removes it). */
export function uiSet(key: string, value: string | null): void {
  if (file) {
    if (value === null) file.delete(key);
    else file.set(key, value);
    void setUiState(key, value);
  }
  localSet(key, value);
}

/** Tests: forget the loaded file. */
export function resetUiStateForTest(): void {
  file = null;
}
