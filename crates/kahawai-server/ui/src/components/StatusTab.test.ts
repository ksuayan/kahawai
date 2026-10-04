import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import StatusTab from "./StatusTab.vue";
import { useSetupStore } from "../stores/setup";
import { formatElapsed, formatWhen, scanTiming } from "../types";

function boot() {
  return mountApp(StatusTab);
}

describe("StatusTab", () => {
  it("shows Stop when running, Start when not", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    await settle();
    expect(wrapper.text()).toContain("Server running");
    expect(wrapper.findAll("button").some((b) => b.text() === "Stop Server")).toBe(true);
    expect(wrapper.findAll("button").some((b) => b.text() === "Start Server")).toBe(false);

    setup.serverStatus = { running: false, bind: "" };
    await settle();
    expect(wrapper.text()).toContain("Server not running");
    expect(wrapper.findAll("button").some((b) => b.text() === "Start Server")).toBe(true);
  });

  it("stops the server", async () => {
    tauri.on("setup_stop_server", undefined);
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Stop Server")!.trigger("click");
    await settle();
    expect(tauri.callsTo("setup_stop_server")).toHaveLength(1);
    expect(setup.serverStatus?.running).toBe(false);
  });

  it("restarts the server", async () => {
    tauri
      .on("setup_restart_server", { running: true, bind: "0.0.0.0:9090" })
      .on("setup_get_running_config", undefined)
      .on("setup_recent_scans", []);
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Restart Server")!.trigger("click");
    await settle();
    expect(setup.serverStatus).toEqual({ running: true, bind: "0.0.0.0:9090" });
  });

  it("shows a live tally and last-added album while a scan is active", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.recentScans = [{ id: "job-0001", kind: "scan", label: "Library scan", progress: 0.4, status: "running" }];
    setup.liveScanStats = { albums: 12, artists: 5, tracks: 130, last_album: "Blue Train", last_album_artist: "John Coltrane" };
    await settle();
    expect(wrapper.text()).toContain("Scanning…");
    expect(wrapper.text()).toContain("12");
    expect(wrapper.text()).toContain("5");
    expect(wrapper.text()).toContain("130");
    expect(wrapper.text()).toContain("Blue Train");
    expect(wrapper.text()).toContain("John Coltrane");
  });

  it("hides the live scan section once nothing is active", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.recentScans = [{ id: "job-0001", kind: "scan", label: "Library scan", progress: 1, status: "done" }];
    await settle();
    expect(wrapper.text()).not.toContain("Scanning…");
  });

  it("shows a failed scan distinctly", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.recentScans = [
      { id: "job-0001", kind: "scan", label: "Library scan", progress: 0, status: "failed", message: "no such file or directory" },
    ];
    await settle();
    expect(wrapper.text()).toContain("Failed");
    expect(wrapper.text()).toContain("no such file or directory");
  });

  it("shows when each scan ran, in local time, and how long it took, failures included", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    const start = Date.UTC(2026, 8, 30, 4, 41, 0);
    setup.recentScans = [
      { id: "job-0002", kind: "scan", label: "Library scan", progress: 1, status: "done", started_at: start, finished_at: start + 12 * 60_000 + 4_000 },
      { id: "job-0001", kind: "scan", label: "Library scan", progress: 0, status: "failed", message: "server restarted", started_at: start - 3_600_000, finished_at: start - 3_600_000 + 45_000 },
    ];
    await settle();
    const timings = wrapper.findAll('[data-testid="scan-timing"]').map((t) => t.text());
    expect(timings).toEqual([
      `${formatWhen(start)} · took 12 min 4 s`,
      `${formatWhen(start - 3_600_000)} · took 45 s`,
    ]);
    expect(wrapper.findAll('[data-testid="recent-scan"]')[1].text()).toContain("server restarted");
  });
});

describe("scan timing text", () => {
  it("formats durations", () => {
    expect([formatElapsed(45_000), formatElapsed(724_000), formatElapsed(3_780_000)]).toEqual(["45 s", "12 min 4 s", "1 h 3 min"]);
  });

  it("counts up while running, waits while queued, and says nothing for an older server", () => {
    const j = { id: "j", kind: "scan" as const, label: "", progress: 0.2 };
    expect(scanTiming({ ...j, status: "running", started_at: 1_000 }, 61_000)).toMatch(/running for 1 min 0 s$/);
    expect(scanTiming({ ...j, status: "queued" }, 0)).toBe("Waiting to start");
    expect(scanTiming({ ...j, status: "done" }, 0)).toBe("");
  });
});

describe("StatusTab: a server that couldn't start", () => {
  it("says why (a port taken by another Kahawai Server)", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = {
      running: false,
      bind: "0.0.0.0:8080",
      error: "Another program is already using 0.0.0.0:8080 (probably another Kahawai Server).",
    };
    await settle();
    expect(wrapper.text()).toContain("Server not running");
    expect(wrapper.get('[data-testid="server-error"]').text()).toContain("already using 0.0.0.0:8080");
  });
});

describe("StatusTab: which server", () => {
  const identity = {
    service: "kahawai-server",
    name: "Kahawai Server",
    version: "0.1.0",
    api_version: 1,
    build: { commit: "cd9b827", dirty: false, built_at: "2026-09-30T19:02:11Z", profile: "debug", target: "x86_64-apple-darwin" },
    catalog_id: "3f1c",
    started_at: Date.UTC(2026, 8, 30, 18, 0, 0),
  };

  it("shows the running server's version and build", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.identity = { ...identity, build: { ...identity.build, profile: "release" } };
    await settle();
    expect(wrapper.get('[data-testid="server-identity"]').text()).toBe(
      "Kahawai Server 0.1.0 · build cd9b827 (release, x86_64-apple-darwin)",
    );
  });

  it("describes another Kahawai Server holding the port, and stops it after a confirmation", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri
      .on("setup_stop_other_server", { running: true, bind: "0.0.0.0:8080", error: null, occupant: null })
      .on("setup_recent_scans", [])
      .on("setup_server_identity", identity);
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: false, bind: "0.0.0.0:8080", error: "Another program is already using 0.0.0.0:8080", occupant: identity };
    await settle();
    const panel = wrapper.get('[data-testid="other-server"]');
    expect(panel.text()).toContain("Another Kahawai Server is running on 0.0.0.0:8080");
    expect(panel.text()).toContain("Kahawai Server 0.1.0 · build cd9b827 (debug, x86_64-apple-darwin)");
    expect(wrapper.find('[data-testid="server-error"]').exists()).toBe(false);

    await panel.findAll("button").find((b) => b.text() === "Stop it and start this server")!.trigger("click");
    expect(tauri.callsTo("setup_stop_other_server")).toHaveLength(0); // asks first
    await wrapper.findAll("button").find((b) => b.text() === "Stop it")!.trigger("click");
    await settle();
    expect(tauri.callsTo("setup_stop_other_server")).toHaveLength(1);
    expect(wrapper.text()).toContain("Server running");
    expect(wrapper.find('[data-testid="other-server"]').exists()).toBe(false);
  });

  it("says so in the panel when the other server won't stop", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri
      .on("setup_stop_other_server", () => {
        throw new Error("The other server is still running after 10 seconds.");
      })
      .on("setup_server_status", { running: false, bind: "0.0.0.0:8080", occupant: identity });
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: false, bind: "0.0.0.0:8080", occupant: identity };
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Stop it and start this server")!.trigger("click");
    await wrapper.findAll("button").find((b) => b.text() === "Stop it")!.trigger("click");
    await settle();
    expect(wrapper.get('[data-testid="other-server"] [role="alert"]').text()).toContain("still running after 10 seconds");
  });
});


describe("StatusTab: scan file counts", () => {
  const job = (files: unknown) => ({
    id: "job-1",
    kind: "scan" as const,
    label: "Library scan",
    progress: 0,
    status: "running" as const,
    started_at: Date.now() - 5000,
    files,
  });

  it("shows only what a first scan knows: files processed and the rate", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.recentScans = [job({ done: 1234, per_sec: 41.2 })] as never;
    await settle();
    expect(wrapper.get('[data-testid="files-processed"]').text()).toBe((1234).toLocaleString());
    expect(wrapper.get('[data-testid="files-rate"]').text()).toBe("41 files/s");
    expect(wrapper.find('[data-testid="files-remaining"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="files-eta"]').exists()).toBe(false);
  });

  it("shows files remaining and a local-time ETA on a rescan", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    const eta = Date.now() + 20 * 60_000;
    setup.recentScans = [job({ done: 1000, total: 5000, per_sec: 8.5, eta_at: eta })] as never;
    await settle();
    expect(wrapper.get('[data-testid="files-remaining"]').text()).toBe((4000).toLocaleString());
    expect(wrapper.get('[data-testid="files-rate"]').text()).toBe("8.5 files/s");
    expect(wrapper.get('[data-testid="files-eta"]').text()).toBe(
      new Date(eta).toLocaleTimeString(undefined, { timeStyle: "short" }),
    );
  });
});

describe("StatusTab: hashing", () => {
  it("shows files remaining, MB/s and an ETA while the hashing job runs", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    const eta = Date.now() + 3 * 3600_000;
    setup.hashJob = {
      id: "job-2",
      kind: "hash_files",
      label: "Content hashing",
      progress: 0.1,
      status: "running",
      files: { done: 800, total: 8000, per_sec: 1.5, mb_per_sec: 112.4, eta_at: eta },
    } as never;
    await settle();
    expect(wrapper.get('[data-testid="hashing"]').text()).toContain("Hashing files…");
    expect(wrapper.get('[data-testid="hash-remaining"]').text()).toBe((7200).toLocaleString());
    expect(wrapper.get('[data-testid="hash-mbps"]').text()).toBe("112 MB/s");
    expect(wrapper.get('[data-testid="hash-eta"]').text()).toBe(
      new Date(eta).toLocaleTimeString(undefined, { timeStyle: "short" }),
    );
  });

  it("is absent when no hashing job is active", async () => {
    const { wrapper } = boot();
    useSetupStore().serverStatus = { running: true, bind: "0.0.0.0:8080" };
    await settle();
    expect(wrapper.find('[data-testid="hashing"]').exists()).toBe(false);
  });
});

describe("StatusTab: heading while the server starts", () => {
  it("says starting, not not running, while the server is opening its catalog", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: false, starting: true, bind: "0.0.0.0:8080" };
    await settle();
    expect(wrapper.text()).toContain("Server starting…");
    expect(wrapper.text()).not.toContain("Server not running");
    setup.serverStatus = { running: true, starting: false, bind: "0.0.0.0:8080" };
    await settle();
    expect(wrapper.text()).toContain("Server running");
  });
});

describe("StatusTab: library panel", () => {
  it("is there when nothing is scanning: the library, the connected Players and Rescan", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.liveScanStats = { albums: 5754, artists: 900, tracks: 80000, audiobooks: 199, players: 2 } as never;
    await settle();
    expect(wrapper.get('[data-testid="scan-heading"]').text()).toBe("Library");
    expect(wrapper.get('[data-testid="books-count"]').text()).toContain("199");
    expect(wrapper.get('[data-testid="players-count"]').text()).toBe("Players connected 2");
    expect(wrapper.find('[data-testid="scan-progress"]').exists()).toBe(false);
    setup.liveScanStats = { albums: 1, artists: 1, tracks: 1, players: 1 } as never;
    await settle();
    expect(wrapper.get('[data-testid="players-count"]').text()).toBe("Player connected 1");
  });

  it("rescans the library or the audiobooks, and says why when it can't", async () => {
    tauri
      .on("setup_rescan_library", undefined)
      .on("setup_recent_scans", [])
      .on("setup_rescan_audiobooks", () => {
        throw new Error("a scan is already running");
      });
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.liveScanStats = { albums: 0, artists: 0, tracks: 0 } as never;
    await settle();
    await wrapper.get('[data-testid="rescan-library"]').trigger("click");
    await settle();
    expect(tauri.callsTo("setup_rescan_library")).toHaveLength(1);
    await wrapper.get('[data-testid="rescan-audiobooks"]').trigger("click");
    await settle();
    expect(wrapper.text()).toContain("a scan is already running");
  });

  it("disables Rescan while a scan runs", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.recentScans = [{ id: "job-1", kind: "scan", label: "Library scan", progress: 0.5, status: "running" }] as never;
    await settle();
    expect((wrapper.get('[data-testid="rescan-library"]').element as HTMLButtonElement).disabled).toBe(true);
    expect(wrapper.get('[data-testid="scan-percent"]').text()).toBe("50%");
  });
});

describe("StatusTab: audiobook scanning", () => {
  it("says it is scanning audiobooks, counts books, and shows the files read", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.recentScans = [
      { id: "job-3", kind: "scan", label: "Audiobook scan", progress: 0.4, status: "running", started_at: Date.now() - 4000, files: { done: 40, total: 100, per_sec: 8, eta_at: Date.now() + 8000 } },
    ] as never;
    setup.liveScanStats = { albums: 100, artists: 20, tracks: 900, audiobooks: 12 } as never;
    await settle();
    expect(wrapper.get('[data-testid="scan-heading"]').text()).toBe("Scanning audiobooks…");
    expect(wrapper.get('[data-testid="books-count"]').text()).toContain("12");
    expect(wrapper.get('[data-testid="files-remaining"]').text()).toBe("60");
    // Progress from the file counts (40 of the previous scan's 100), with its ETA.
    expect(wrapper.get('[data-testid="scan-percent"]').text()).toBe("40%");
    expect(wrapper.find('[data-testid="files-eta"]').exists()).toBe(true);
  });

  it("a music scan keeps its tally; no progress bar until there is something to measure", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.recentScans = [{ id: "job-1", kind: "scan", label: "Library scan", progress: 0, status: "running", started_at: Date.now() }] as never;
    setup.liveScanStats = { albums: 5, artists: 2, tracks: 40, audiobooks: 0 } as never;
    await settle();
    expect(wrapper.get('[data-testid="scan-heading"]').text()).toBe("Scanning…");
    expect(wrapper.text()).toContain("Albums");
    expect(wrapper.find('[data-testid="scan-progress"]').exists()).toBe(false);
  });

  it("shows the online details lookup while it runs", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.bookLookupJob = { id: "job-9", kind: "enrich_books", label: "Audiobook info lookup", progress: 0.25, status: "running" } as never;
    await settle();
    expect(wrapper.get('[data-testid="lookup-percent"]').text()).toBe("25%");
    setup.bookLookupJob = null;
    await settle();
    expect(wrapper.find('[data-testid="book-lookup"]').exists()).toBe(false);
  });
});
