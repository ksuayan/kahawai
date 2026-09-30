import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  setupApplyConfig,
  setupGetRunningConfig,
  setupGetState,
  setupLiveScanStats,
  setupPickDirectory,
  setupQuit,
  setupRecentScans,
  setupRestartServer,
  setupRevealConfig,
  setupSaveConfig,
  setupServerIdentity,
  setupServerStatus,
  setupStartServer,
  setupStopOtherServer,
  setupStopServer,
  setupValidateDir,
} from "../tauri";
import type { LiveScanStats, MusicDirEntry, ScanJob, ServerIdentity, ServerStatus } from "../types";
import { dirStatus, isJobActive } from "../types";

/** A `host:port` shape good enough to gate the Continue button; the backend
 *  re-validates authoritatively with `SocketAddr::parse`. */
const BIND_RE = /^[^\s:]+:\d{1,5}$/;

export const useSetupStore = defineStore("setup", () => {
  const loading = ref(true);
  /** "wizard" until a usable config exists, then the minimal status view. */
  const view = ref<"wizard" | "status">("wizard");
  const step = ref(0);
  /** The About dialog (the app menu's About item, or the About button). */
  const aboutOpen = ref(false);
  /** Which tab the status view shows. */
  const activeTab = ref<"status" | "settings">("status");

  const configPath = ref("");
  const dirs = ref<MusicDirEntry[]>([]);
  const dbDir = ref("");
  const bind = ref("0.0.0.0:8080");

  const saveError = ref<string | null>(null);
  const startError = ref<string | null>(null);
  const serverStatus = ref<ServerStatus | null>(null);
  /** This app's running server's identity (version, build). */
  const identity = ref<ServerIdentity | null>(null);
  const stoppingOther = ref(false);
  const dbDirValidation = ref<MusicDirEntry["validation"]>(undefined);

  // Status/Settings tabs: the running server's own config, edited and
  // applied live (no restart) — distinct from `dirs`/`bind`/`dbDir` above,
  // which are the wizard's own working copy before a server exists.
  const runningDbPath = ref("");
  const runningBind = ref("");
  /** Display copy: the live config's folders, with pending adds/removes
   *  already reflected so the list looks right immediately. The *only*
   *  things actually sent to the backend are `pendingAdds`/`pendingRemoves`
   *  below — never this full list — so a folder can never be dropped except
   *  by explicitly removing it (see `setup_apply_config`). */
  const runningDirs = ref<MusicDirEntry[]>([]);
  const pendingAdds = ref<string[]>([]);
  const pendingRemoves = ref<string[]>([]);
  const applying = ref(false);
  const applyError = ref<string | null>(null);
  const recentScans = ref<ScanJob[]>([]);
  const liveScanStats = ref<LiveScanStats | null>(null);
  let scanPollTimer: number | undefined;

  const okDirCount = computed(
    () => dirs.value.filter((d) => d.validation && dirStatus(d.validation) === "ok").length,
  );
  const canLeaveFolders = computed(() => okDirCount.value >= 1);
  const canLeaveDatabase = computed(() => dbDirValidation.value?.writable === true);
  const bindLooksValid = computed(() => BIND_RE.test(bind.value.trim()));
  const canApply = computed(() => pendingAdds.value.length > 0 || pendingRemoves.value.length > 0);
  /** True while any scan job (however it was started — Settings' Apply, the
   *  startup scan, anything) is queued or running. Drives the Status tab's
   *  live view independent of who triggered it. */
  const isScanning = computed(() => recentScans.value.some(isJobActive));

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
      if (isScanning.value) ensureScanPolling();
    } else {
      view.value = "wizard";
      step.value = 0;
    }
    loading.value = false;
  }

  /** Populates the Status/Settings tabs' folder list from the *running*
   *  server's config (not the wizard's `dirs`, which is only meaningful
   *  before a server exists) — and clears any pending add/remove, since
   *  this re-syncs to the authoritative source. */
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
    pendingAdds.value = [];
    pendingRemoves.value = [];
  }

  /** Adds to the display list and queues the folder to actually be added on
   *  the next Apply. Re-adding something removed earlier this session just
   *  cancels that pending removal — it was never actually gone. */
  async function addRunningDirFromPicker(): Promise<void> {
    const picked = await setupPickDirectory();
    if (!picked) return;
    if (runningDirs.value.some((d) => d.path === picked)) return;
    runningDirs.value.push({ path: picked, validating: true });
    if (pendingRemoves.value.includes(picked)) {
      pendingRemoves.value = pendingRemoves.value.filter((p) => p !== picked);
    } else {
      pendingAdds.value.push(picked);
    }
    const validation = await setupValidateDir(picked);
    const row = runningDirs.value.find((d) => d.path === picked);
    if (row) {
      row.validation = validation;
      row.validating = false;
    }
  }

  /** Removes from the display list. If the folder was only a pending add
   *  (never actually applied), that's all — otherwise queues an explicit
   *  removal, since that's the only way `setup_apply_config` will drop it. */
  function removeRunningDir(path: string): void {
    runningDirs.value = runningDirs.value.filter((d) => d.path !== path);
    if (pendingAdds.value.includes(path)) {
      pendingAdds.value = pendingAdds.value.filter((p) => p !== path);
    } else if (!pendingRemoves.value.includes(path)) {
      pendingRemoves.value.push(path);
    }
  }

  async function loadRecentScans(): Promise<void> {
    recentScans.value = await setupRecentScans();
  }

  async function refreshLiveScanStats(): Promise<void> {
    liveScanStats.value = await setupLiveScanStats();
  }

  /** Poll while a scan is active, for the Status tab's live tally and the
   *  Settings tab's Apply button — regardless of who started the scan.
   *  Safe to call repeatedly; a second call while already polling is a
   *  no-op. */
  function ensureScanPolling(): void {
    if (scanPollTimer !== undefined) return;
    scanPollTimer = window.setInterval(async () => {
      await Promise.all([loadRecentScans(), refreshLiveScanStats()]);
      if (!isScanning.value) {
        window.clearInterval(scanPollTimer);
        scanPollTimer = undefined;
      }
    }, 1000);
  }

  /** Add/remove music folders on the running server: applies immediately
   *  (no restart) and triggers a rescan, which — on success — also tells
   *  any connected Player clients the catalog changed (SSE). */
  async function applyAndRescan(): Promise<void> {
    applyError.value = null;
    applying.value = true;
    try {
      await setupApplyConfig({ add: pendingAdds.value, remove: pendingRemoves.value });
      pendingAdds.value = [];
      pendingRemoves.value = [];
      await loadRecentScans();
      ensureScanPolling();
    } catch (err) {
      applyError.value = String(err);
    } finally {
      applying.value = false;
    }
  }

  /** Stops the server process (not the app). */
  async function stopServer(): Promise<void> {
    await setupStopServer();
    serverStatus.value = { running: false, bind: serverStatus.value?.bind ?? "" };
  }

  /** Stops, then starts again from the on-disk config — e.g. after an
   *  Advanced settings change (bind/database) that needs a restart. */
  async function loadIdentity(): Promise<void> {
    identity.value = serverStatus.value?.running ? ((await setupServerIdentity()) ?? null) : null;
  }

  /** Another Kahawai Server holds the port: stop it, then start this one. */
  async function stopOtherServer(): Promise<void> {
    startError.value = null;
    stoppingOther.value = true;
    try {
      serverStatus.value = await setupStopOtherServer();
      await Promise.all([loadRunningConfig(), loadRecentScans(), loadIdentity()]);
      if (isScanning.value) ensureScanPolling();
    } catch (err) {
      startError.value = String(err);
      serverStatus.value = (await setupServerStatus()) ?? serverStatus.value;
    } finally {
      stoppingOther.value = false;
    }
  }

  async function restartServer(): Promise<void> {
    startError.value = null;
    try {
      serverStatus.value = await setupRestartServer();
      await Promise.all([loadRunningConfig(), loadRecentScans(), loadIdentity()]);
      if (isScanning.value) ensureScanPolling();
    } catch (err) {
      startError.value = String(err);
      // Why it couldn't start, and who holds the port if that's why.
      serverStatus.value = (await setupServerStatus()) ?? serverStatus.value;
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
      void loadIdentity();
      view.value = "status";
      // Without this, `runningDirs` stays empty (its initial value) until
      // something else happens to call `loadRunningConfig()` — and the
      // Status view's "Add folder" + Apply pushes onto whatever
      // `runningDirs` already holds, then *overwrites* the on-disk
      // `music_dirs` with exactly that list. Skipping this seed step meant
      // the very folder(s) just set up in the wizard could be silently
      // dropped the first time someone added another one from Status.
      await Promise.all([loadRunningConfig(), loadRecentScans()]);
      if (isScanning.value) ensureScanPolling();
    } catch (err) {
      startError.value = String(err);
      serverStatus.value = (await setupServerStatus()) ?? serverStatus.value;
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
    activeTab,
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
    pendingAdds,
    pendingRemoves,
    applying,
    applyError,
    recentScans,
    liveScanStats,
    canApply,
    isScanning,
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
    refreshLiveScanStats,
    applyAndRescan,
    stopServer,
    restartServer,
    identity,
    aboutOpen,
    stoppingOther,
    loadIdentity,
    stopOtherServer,
  };
});
