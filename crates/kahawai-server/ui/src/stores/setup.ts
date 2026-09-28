import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  setupApplyConfig,
  setupGetRunningConfig,
  setupGetState,
  setupPickDirectory,
  setupQuit,
  setupRecentScans,
  setupRevealConfig,
  setupSaveConfig,
  setupServerStatus,
  setupStartServer,
  setupValidateDir,
} from "../tauri";
import type { MusicDirEntry, ScanJob, ServerStatus } from "../types";
import { dirStatus, isJobActive } from "../types";

/** A `host:port` shape good enough to gate the Continue button; the backend
 *  re-validates authoritatively with `SocketAddr::parse`. */
const BIND_RE = /^[^\s:]+:\d{1,5}$/;

export const useSetupStore = defineStore("setup", () => {
  const loading = ref(true);
  /** "wizard" until a usable config exists, then the minimal status view. */
  const view = ref<"wizard" | "status">("wizard");
  const step = ref(0);

  const configPath = ref("");
  const dirs = ref<MusicDirEntry[]>([]);
  const dbDir = ref("");
  const bind = ref("0.0.0.0:8080");

  const saveError = ref<string | null>(null);
  const startError = ref<string | null>(null);
  const serverStatus = ref<ServerStatus | null>(null);
  const dbDirValidation = ref<MusicDirEntry["validation"]>(undefined);

  // Status view: the running server's own config, edited and applied live
  // (no restart) — distinct from `dirs`/`bind`/`dbDir` above, which are the
  // wizard's own working copy before a server exists.
  const runningDbPath = ref("");
  const runningBind = ref("");
  const runningDirs = ref<MusicDirEntry[]>([]);
  const applying = ref(false);
  const applyError = ref<string | null>(null);
  const recentScans = ref<ScanJob[]>([]);
  let scanPollTimer: number | undefined;

  const okDirCount = computed(
    () => dirs.value.filter((d) => d.validation && dirStatus(d.validation) === "ok").length,
  );
  const canLeaveFolders = computed(() => okDirCount.value >= 1);
  const canLeaveDatabase = computed(() => dbDirValidation.value?.writable === true);
  const bindLooksValid = computed(() => BIND_RE.test(bind.value.trim()));
  const canApply = computed(
    () => runningDirs.value.some((d) => d.validation && dirStatus(d.validation) === "ok"),
  );

  /** Called once on mount: decides wizard vs. status, prefills from any
   *  existing config. */
  async function init(): Promise<void> {
    loading.value = true;
    const state = await setupGetState();
    configPath.value = state?.config_path ?? "";
    if (state?.config_exists && state.config) {
      dirs.value = state.config.music_dirs.map((path) => ({ path, validating: false }));
      bind.value = state.config.bind;
      const dbPath = state.config.db_path;
      dbDir.value = dbPath.includes("/") ? dbPath.slice(0, dbPath.lastIndexOf("/")) : dbPath;
      view.value = "status";
      serverStatus.value = (await setupServerStatus()) ?? null;
      await Promise.all([loadRunningConfig(), loadRecentScans()]);
    } else {
      view.value = "wizard";
      step.value = 0;
    }
    loading.value = false;
  }

  /** Populates the Status view's editable folder list from the *running*
   *  server's config (not the wizard's `dirs`, which is only meaningful
   *  before a server exists). */
  async function loadRunningConfig(): Promise<void> {
    const config = await setupGetRunningConfig();
    if (!config) return;
    runningDbPath.value = config.db_path;
    runningBind.value = config.bind;
    const validations = await Promise.all(config.music_dirs.map((path) => setupValidateDir(path)));
    runningDirs.value = config.music_dirs.map((path, i) => ({
      path,
      validating: false,
      validation: validations[i],
    }));
  }

  async function addRunningDirFromPicker(): Promise<void> {
    const picked = await setupPickDirectory();
    if (!picked) return;
    if (runningDirs.value.some((d) => d.path === picked)) return;
    runningDirs.value.push({ path: picked, validating: true });
    const validation = await setupValidateDir(picked);
    const row = runningDirs.value.find((d) => d.path === picked);
    if (row) {
      row.validation = validation;
      row.validating = false;
    }
  }

  function removeRunningDir(path: string): void {
    runningDirs.value = runningDirs.value.filter((d) => d.path !== path);
  }

  async function loadRecentScans(): Promise<void> {
    recentScans.value = await setupRecentScans();
  }

  /** Poll `setup_recent_scans` until the newest scan job settles, so the UI
   *  can show "scanning…" and then the final result without the caller
   *  needing its own timer. Safe to call again — restarts the poll. */
  function pollScansUntilSettled(): void {
    window.clearInterval(scanPollTimer);
    scanPollTimer = window.setInterval(async () => {
      await loadRecentScans();
      const newest = recentScans.value[0];
      if (!newest || !isJobActive(newest)) {
        window.clearInterval(scanPollTimer);
        scanPollTimer = undefined;
        applying.value = false;
      }
    }, 500);
  }

  /** Add/remove music folders on the running server: applies immediately
   *  (no restart) and triggers a rescan, which — on success — also tells
   *  any connected Player clients the catalog changed (SSE). */
  async function applyAndRescan(): Promise<void> {
    applyError.value = null;
    applying.value = true;
    try {
      await setupApplyConfig({
        music_dirs: runningDirs.value
          .filter((d) => d.validation && dirStatus(d.validation) === "ok")
          .map((d) => d.path),
      });
      pollScansUntilSettled();
    } catch (err) {
      applyError.value = String(err);
      applying.value = false;
    }
  }

  async function addDirFromPicker(): Promise<void> {
    const picked = await setupPickDirectory();
    if (!picked) return;
    if (dirs.value.some((d) => d.path === picked)) return;
    dirs.value.push({ path: picked, validating: true });
    const validation = await setupValidateDir(picked);
    // Mutate through the reactive array, not the plain object captured
    // above — Vue's proxy only notifies watchers on sets made via itself.
    const row = dirs.value.find((d) => d.path === picked);
    if (row) {
      row.validation = validation;
      row.validating = false;
    }
  }

  function removeDir(path: string): void {
    dirs.value = dirs.value.filter((d) => d.path !== path);
  }

  async function pickDbDir(): Promise<void> {
    const picked = await setupPickDirectory();
    if (!picked) return;
    dbDir.value = picked;
    dbDirValidation.value = await setupValidateDir(picked);
  }

  function goNext(): void {
    step.value = Math.min(4, step.value + 1);
  }

  function goBack(): void {
    step.value = Math.max(0, step.value - 1);
  }

  async function save(): Promise<boolean> {
    saveError.value = null;
    try {
      await setupSaveConfig({
        music_dirs: dirs.value
          .filter((d) => d.validation && dirStatus(d.validation) === "ok")
          .map((d) => d.path),
        db_dir: dbDir.value,
        bind: bind.value.trim(),
      });
      return true;
    } catch (err) {
      saveError.value = String(err);
      return false;
    }
  }

  async function startServerAndContinue(): Promise<void> {
    startError.value = null;
    try {
      serverStatus.value = await setupStartServer();
      view.value = "status";
      // Without this, `runningDirs` stays empty (its initial value) until
      // something else happens to call `loadRunningConfig()` — and the
      // Status view's "Add folder" + Apply pushes onto whatever
      // `runningDirs` already holds, then *overwrites* the on-disk
      // `music_dirs` with exactly that list. Skipping this seed step meant
      // the very folder(s) just set up in the wizard could be silently
      // dropped the first time someone added another one from Status.
      await Promise.all([loadRunningConfig(), loadRecentScans()]);
    } catch (err) {
      startError.value = String(err);
    }
  }

  async function editConfiguration(): Promise<void> {
    await init();
    view.value = "wizard";
    step.value = 1;
  }

  async function revealConfig(): Promise<void> {
    await setupRevealConfig();
  }

  async function quit(): Promise<void> {
    await setupQuit();
  }

  return {
    loading,
    view,
    step,
    configPath,
    dirs,
    dbDir,
    bind,
    saveError,
    startError,
    serverStatus,
    dbDirValidation,
    okDirCount,
    canLeaveFolders,
    canLeaveDatabase,
    bindLooksValid,
    runningDbPath,
    runningBind,
    runningDirs,
    applying,
    applyError,
    recentScans,
    canApply,
    init,
    addDirFromPicker,
    removeDir,
    pickDbDir,
    goNext,
    goBack,
    save,
    startServerAndContinue,
    editConfiguration,
    revealConfig,
    quit,
    loadRunningConfig,
    addRunningDirFromPicker,
    removeRunningDir,
    loadRecentScans,
    applyAndRescan,
  };
});
