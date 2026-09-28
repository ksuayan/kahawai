import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { dialog } from "../test/dialog-mock";
import { useSetupStore } from "./setup";

function inTauri(): void {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

const okValidation = { exists: true, is_dir: true, readable: true, writable: true, audio_files: 12, truncated: false };
const emptyValidation = { exists: true, is_dir: true, readable: true, writable: true, audio_files: 0, truncated: false };
const badValidation = { exists: false, is_dir: false, readable: false, writable: false, audio_files: 0, truncated: false };

beforeEach(() => {
  setActivePinia(createPinia());
});

describe("setup store: init", () => {
  it("opens the wizard at step 0 when no usable config exists", async () => {
    tauri.on("setup_get_state", { config_path: "/cfg/config.toml", config_exists: false });
    const setup = useSetupStore();
    await setup.init();
    expect(setup.view).toBe("wizard");
    expect(setup.step).toBe(0);
    expect(setup.configPath).toBe("/cfg/config.toml");
    expect(setup.loading).toBe(false);
  });

  it("prefills from an existing config and shows the status view", async () => {
    tauri
      .on("setup_get_state", {
        config_path: "/cfg/config.toml",
        config_exists: true,
        config: {
          music_dirs: ["/music/a", "/music/b"],
          bind: "127.0.0.1:9090",
          db_path: "/data/music.db",
          preferred_ladder: ["passthrough", "flac"],
          dsd_story: "pcm",
          scan_on_startup: false,
        },
      })
      .on("setup_server_status", { running: true, bind: "127.0.0.1:9090" });
    const setup = useSetupStore();
    await setup.init();
    expect(setup.view).toBe("status");
    expect(setup.dirs.map((d) => d.path)).toEqual(["/music/a", "/music/b"]);
    expect(setup.bind).toBe("127.0.0.1:9090");
    expect(setup.dbDir).toBe("/data");
    expect(setup.serverStatus).toEqual({ running: true, bind: "127.0.0.1:9090" });
  });
});

describe("setup store: music folders", () => {
  it("adds a picked directory and validates it", async () => {
    inTauri();
    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    const setup = useSetupStore();
    await setup.addDirFromPicker();
    expect(setup.dirs).toHaveLength(1);
    expect(setup.dirs[0]).toMatchObject({ path: "/music/new", validating: false, validation: okValidation });
    expect(setup.canLeaveFolders).toBe(true);
  });

  it("does not add a duplicate path", async () => {
    inTauri();
    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    const setup = useSetupStore();
    await setup.addDirFromPicker();
    await setup.addDirFromPicker();
    expect(setup.dirs).toHaveLength(1);
  });

  it("ignores a cancelled picker", async () => {
    inTauri();
    dialog.nextPath = null;
    const setup = useSetupStore();
    await setup.addDirFromPicker();
    expect(setup.dirs).toHaveLength(0);
  });

  it("does not count an inaccessible or empty folder toward canLeaveFolders", async () => {
    inTauri();
    const setup = useSetupStore();
    dialog.nextPath = "/music/empty";
    tauri.on("setup_validate_dir", emptyValidation);
    await setup.addDirFromPicker();
    expect(setup.canLeaveFolders).toBe(false);

    dialog.nextPath = "/music/bad";
    tauri.on("setup_validate_dir", badValidation);
    await setup.addDirFromPicker();
    expect(setup.canLeaveFolders).toBe(false);

    dialog.nextPath = "/music/good";
    tauri.on("setup_validate_dir", okValidation);
    await setup.addDirFromPicker();
    expect(setup.canLeaveFolders).toBe(true);
    expect(setup.okDirCount).toBe(1);
  });

  it("removes a directory by path", async () => {
    inTauri();
    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    const setup = useSetupStore();
    await setup.addDirFromPicker();
    setup.removeDir("/music/new");
    expect(setup.dirs).toHaveLength(0);
  });
});

describe("setup store: database step", () => {
  it("requires the picked folder to be writable", async () => {
    inTauri();
    const setup = useSetupStore();
    dialog.nextPath = "/data/readonly";
    tauri.on("setup_validate_dir", { ...okValidation, writable: false });
    await setup.pickDbDir();
    expect(setup.canLeaveDatabase).toBe(false);

    dialog.nextPath = "/data/writable";
    tauri.on("setup_validate_dir", okValidation);
    await setup.pickDbDir();
    expect(setup.canLeaveDatabase).toBe(true);
  });
});

describe("setup store: bind address validation", () => {
  it("accepts host:port and rejects malformed values", () => {
    const setup = useSetupStore();
    setup.bind = "0.0.0.0:8080";
    expect(setup.bindLooksValid).toBe(true);
    setup.bind = "not-an-address";
    expect(setup.bindLooksValid).toBe(false);
    setup.bind = "192.168.1.10:";
    expect(setup.bindLooksValid).toBe(false);
  });
});

describe("setup store: save", () => {
  it("saves the current selections and clears any prior error", async () => {
    tauri.on("setup_save_config", undefined);
    const setup = useSetupStore();
    setup.dirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    setup.dbDir = "/data";
    setup.bind = "0.0.0.0:8080";
    const ok = await setup.save();
    expect(ok).toBe(true);
    expect(setup.saveError).toBeNull();
    expect(tauri.callsTo("setup_save_config")).toEqual([
      { input: { music_dirs: ["/music/a"], db_dir: "/data", bind: "0.0.0.0:8080" } },
    ]);
  });

  it("surfaces the backend's error string on failure", async () => {
    tauri.on("setup_save_config", () => {
      throw new Error("bind address already in use");
    });
    const setup = useSetupStore();
    const ok = await setup.save();
    expect(ok).toBe(false);
    expect(setup.saveError).toContain("bind address already in use");
  });
});

describe("setup store: start server", () => {
  it("switches to the status view on a successful start", async () => {
    tauri.on("setup_start_server", { running: true, bind: "0.0.0.0:8080" });
    const setup = useSetupStore();
    await setup.startServerAndContinue();
    expect(setup.view).toBe("status");
    expect(setup.serverStatus).toEqual({ running: true, bind: "0.0.0.0:8080" });
    expect(setup.startError).toBeNull();
  });

  it("stays put and records the error on a bind failure", async () => {
    tauri.on("setup_start_server", () => {
      throw new Error("address already in use");
    });
    const setup = useSetupStore();
    setup.view = "wizard";
    await setup.startServerAndContinue();
    expect(setup.view).toBe("wizard");
    expect(setup.startError).toContain("address already in use");
  });

  it("seeds the Status view's folder list from the config it just started with", async () => {
    tauri
      .on("setup_start_server", { running: true, bind: "0.0.0.0:8080" })
      .on("setup_get_running_config", {
        music_dirs: ["/music/a"],
        bind: "0.0.0.0:8080",
        db_path: "/data/music.db",
        preferred_ladder: ["passthrough", "flac"],
        dsd_story: "pcm",
        scan_on_startup: true,
      })
      .on("setup_validate_dir", okValidation);
    const setup = useSetupStore();
    await setup.startServerAndContinue();
    expect(setup.runningDirs.map((d) => d.path)).toEqual(["/music/a"]);
  });

  // Regression: `runningDirs` used to stay empty after finishing the wizard
  // (only `init()`'s status branch seeded it, and this path bypasses that),
  // so adding a folder from the Status view right after finishing the
  // wizard would push onto an empty list — then Apply overwrote the on-disk
  // `music_dirs` with *only* the newly added folder, silently dropping the
  // one the wizard had just set up.
  it("does not drop the folder just set up in the wizard when Apply is used right after starting", async () => {
    inTauri();
    tauri
      .on("setup_start_server", { running: true, bind: "0.0.0.0:8080" })
      .on("setup_get_running_config", {
        music_dirs: ["/music/a"],
        bind: "0.0.0.0:8080",
        db_path: "/data/music.db",
        preferred_ladder: ["passthrough", "flac"],
        dsd_story: "pcm",
        scan_on_startup: true,
      })
      .on("setup_validate_dir", okValidation)
      .on("setup_apply_config", undefined)
      .on("setup_recent_scans", []);
    const setup = useSetupStore();
    await setup.startServerAndContinue();

    dialog.nextPath = "/music/b";
    await setup.addRunningDirFromPicker();
    vi.useFakeTimers();
    await setup.applyAndRescan();
    await vi.advanceTimersByTimeAsync(1000); // let the settle-poll clear itself
    vi.useRealTimers();

    // Only the delta is sent — the backend merges it against the *live*
    // config, so "/music/a" (never touched) can't be dropped even if this
    // payload didn't repeat it.
    expect(tauri.callsTo("setup_apply_config")).toEqual([{ input: { add: ["/music/b"], remove: [] } }]);
  });
});

describe("setup store: editConfiguration", () => {
  it("re-fetches state, jumps to the wizard's music-folders step", async () => {
    tauri.on("setup_get_state", {
      config_path: "/cfg/config.toml",
      config_exists: true,
      config: {
        music_dirs: ["/music/a"],
        bind: "0.0.0.0:8080",
        db_path: "/data/music.db",
        preferred_ladder: ["passthrough", "flac"],
        dsd_story: "pcm",
        scan_on_startup: false,
      },
    });
    tauri.on("setup_server_status", { running: true, bind: "0.0.0.0:8080" });
    const setup = useSetupStore();
    await setup.editConfiguration();
    expect(setup.view).toBe("wizard");
    expect(setup.step).toBe(1);
    expect(setup.dirs.map((d) => d.path)).toEqual(["/music/a"]);
  });
});

describe("setup store: live config (running server)", () => {
  it("prefills the running folder list from the live config, not the wizard's", async () => {
    tauri
      .on("setup_get_running_config", {
        music_dirs: ["/music/a", "/music/b"],
        bind: "0.0.0.0:8080",
        db_path: "/data/music.db",
        preferred_ladder: ["passthrough", "flac"],
        dsd_story: "pcm",
        scan_on_startup: true,
      })
      .on("setup_validate_dir", okValidation);
    const setup = useSetupStore();
    await setup.loadRunningConfig();
    expect(setup.runningDirs.map((d) => d.path)).toEqual(["/music/a", "/music/b"]);
    expect(setup.runningDbPath).toBe("/data/music.db");
    expect(setup.runningBind).toBe("0.0.0.0:8080");
    // Loading re-syncs to the authoritative source, so there's nothing
    // pending yet — canApply only turns on once something's actually added
    // or removed.
    expect(setup.canApply).toBe(false);
  });

  it("adds a picked folder to the running list via the same picker as the wizard", async () => {
    inTauri();
    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    const setup = useSetupStore();
    await setup.addRunningDirFromPicker();
    expect(setup.runningDirs).toHaveLength(1);
    expect(setup.runningDirs[0]).toMatchObject({ path: "/music/new", validation: okValidation });
  });

  it("removes a running folder by path", () => {
    const setup = useSetupStore();
    setup.runningDirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    setup.removeRunningDir("/music/a");
    expect(setup.runningDirs).toHaveLength(0);
  });

  it("applies the pending add, then polls until the rescan settles", async () => {
    vi.useFakeTimers();
    inTauri();
    tauri.on("setup_apply_config", undefined);
    const setup = useSetupStore();
    dialog.nextPath = "/music/a";
    tauri.on("setup_validate_dir", okValidation);
    await setup.addRunningDirFromPicker();

    tauri.on("setup_recent_scans", [
      { id: "job-0002", kind: "scan", label: "Library scan", progress: 0.5, status: "running" },
    ]);
    const applied = setup.applyAndRescan();
    await vi.advanceTimersByTimeAsync(0); // let setupApplyConfig's await resolve
    await applied;
    expect(setup.applying).toBe(false); // resolves once the request itself is done
    expect(setup.isScanning).toBe(true); // still true from the freshly-loaded job list
    expect(tauri.callsTo("setup_apply_config")).toEqual([{ input: { add: ["/music/a"], remove: [] } }]);
    expect(setup.pendingAdds).toEqual([]);

    tauri.on("setup_recent_scans", [
      {
        id: "job-0002",
        kind: "scan",
        label: "Library scan",
        progress: 1,
        status: "done",
        message: "scan complete: 3 added",
      },
    ]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(setup.isScanning).toBe(false);
    expect(setup.recentScans[0]).toMatchObject({ status: "done" });
    vi.useRealTimers();
  });

  it("surfaces the backend's error and stops without polling on failure", async () => {
    tauri.on("setup_apply_config", () => {
      throw new Error("the server is not running");
    });
    const setup = useSetupStore();
    setup.runningDirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    await setup.applyAndRescan();
    expect(setup.applying).toBe(false);
    expect(setup.applyError).toContain("the server is not running");
  });
});
