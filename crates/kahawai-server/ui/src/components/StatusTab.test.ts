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

