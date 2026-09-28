import { describe, expect, it, vi } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import { dialog } from "../test/dialog-mock";
import StatusView from "./StatusView.vue";
import { useSetupStore } from "../stores/setup";

const okValidation = { exists: true, is_dir: true, readable: true, writable: true, audio_files: 3 };

function boot() {
  return mountApp(StatusView);
}

describe("StatusView", () => {
  it("shows running music folders and lets you remove one", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    setup.runningDirs = [
      { path: "/music/a", validating: false, validation: okValidation },
      { path: "/music/b", validating: false, validation: okValidation },
    ];
    await settle();
    expect(wrapper.text()).toContain("/music/a");
    expect(wrapper.text()).toContain("/music/b");

    await wrapper.findAll("button").find((b) => b.attributes("aria-label") === "Remove folder")!.trigger("click");
    expect(setup.runningDirs.map((d) => d.path)).toEqual(["/music/b"]);
  });

  it("adds a folder via the picker and applies, showing the scan result", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.on("setup_apply_config", undefined).on("setup_recent_scans", [
      { id: "job-0001", kind: "scan", label: "Library scan", progress: 1, status: "done", message: "scan complete: 3 added" },
    ]);
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };

    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    await wrapper.findAll("button").find((b) => b.text() === "Add folder…")!.trigger("click");
    await settle();
    expect(wrapper.text()).toContain("/music/new");

    vi.useFakeTimers();
    await wrapper.findAll("button").find((b) => b.text() === "Apply")!.trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(500);
    vi.useRealTimers();
    expect(tauri.callsTo("setup_apply_config")).toEqual([{ input: { music_dirs: ["/music/new"] } }]);
    expect(wrapper.text()).toContain("scan complete: 3 added");
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

  it("disables Apply until a folder validates ok", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.serverStatus = { running: true, bind: "0.0.0.0:8080" };
    await settle();
    const applyBtn = () => wrapper.findAll("button").find((b) => b.text() === "Apply")!;
    expect((applyBtn().element as HTMLButtonElement).disabled).toBe(true);

    setup.runningDirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    await settle();
    expect((applyBtn().element as HTMLButtonElement).disabled).toBe(false);
  });
});
