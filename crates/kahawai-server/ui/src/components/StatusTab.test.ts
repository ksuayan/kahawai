import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import StatusTab from "./StatusTab.vue";
import { useSetupStore } from "../stores/setup";

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
});
