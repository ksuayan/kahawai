import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import { dialog } from "../test/dialog-mock";
import SettingsTab from "./SettingsTab.vue";
import { useSetupStore } from "../stores/setup";

const okValidation = { exists: true, is_dir: true, readable: true, writable: true, audio_files: 3, truncated: false };

function boot() {
  return mountApp(SettingsTab);
}

function inTauri(): void {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

describe("SettingsTab", () => {
  it("shows the running folder list and Apply is disabled with no pending changes", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.runningDirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    await settle();
    expect(wrapper.text()).toContain("/music/a");
    const applyBtn = wrapper.findAll("button").find((b) => b.text() === "Apply")!;
    expect((applyBtn.element as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows a truncated folder's count as a lower bound with an explanatory tooltip", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.runningDirs = [
      { path: "/Volumes/NetMusic", validating: false, validation: { ...okValidation, audio_files: 125_000, truncated: true } },
    ];
    await settle();
    expect(wrapper.text()).toContain("125,000+ audio files");
    const chip = wrapper.findAll("div").find((d) => d.text() === "125,000+ audio files")!;
    expect(chip.attributes("title")).toContain("lower bound");
  });

  it("adding a folder marks it 'new' and enables Apply", async () => {
    inTauri();
    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    const { wrapper } = boot();
    const setup = useSetupStore();
    await wrapper.findAll("button").find((b) => b.text() === "Add folder…")!.trigger("click");
    await settle();
    expect(wrapper.text()).toContain("/music/new");
    expect(wrapper.text()).toContain("new");
    expect(setup.pendingAdds).toEqual(["/music/new"]);
    const applyBtn = wrapper.findAll("button").find((b) => b.text() === "Apply")!;
    expect((applyBtn.element as HTMLButtonElement).disabled).toBe(false);
  });

  it("removing an already-live folder queues an explicit removal", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.runningDirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    await settle();
    await wrapper.findAll("button").find((b) => b.attributes("aria-label") === "Remove folder")!.trigger("click");
    expect(setup.runningDirs).toHaveLength(0);
    expect(setup.pendingRemoves).toEqual(["/music/a"]);
  });

  it("removing a not-yet-applied add just cancels it (no removal queued)", async () => {
    inTauri();
    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    const { wrapper } = boot();
    const setup = useSetupStore();
    await wrapper.findAll("button").find((b) => b.text() === "Add folder…")!.trigger("click");
    await settle();
    await wrapper.findAll("button").find((b) => b.attributes("aria-label") === "Remove folder")!.trigger("click");
    expect(setup.pendingAdds).toEqual([]);
    expect(setup.pendingRemoves).toEqual([]);
  });

  it("Apply sends only the add/remove deltas, never the full list, and clears them on success", async () => {
    inTauri();
    tauri.on("setup_apply_config", undefined).on("setup_recent_scans", []);
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.runningDirs = [{ path: "/music/a", validating: false, validation: okValidation }];

    dialog.nextPath = "/music/b";
    tauri.on("setup_validate_dir", okValidation);
    await wrapper.findAll("button").find((b) => b.text() === "Add folder…")!.trigger("click");
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Apply")!.trigger("click");
    await settle();

    expect(tauri.callsTo("setup_apply_config")).toEqual([{ input: { add: ["/music/b"], remove: [] } }]);
    expect(setup.pendingAdds).toEqual([]);
    expect(setup.pendingRemoves).toEqual([]);
  });

  it("shows the backend's error and keeps the pending change on failure", async () => {
    inTauri();
    tauri.on("setup_apply_config", () => {
      throw new Error("At least one music folder is required.");
    });
    dialog.nextPath = "/music/new";
    tauri.on("setup_validate_dir", okValidation);
    const { wrapper } = boot();
    const setup = useSetupStore();
    await wrapper.findAll("button").find((b) => b.text() === "Add folder…")!.trigger("click");
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Apply")!.trigger("click");
    await settle();
    expect(wrapper.text()).toContain("At least one music folder is required.");
    expect(setup.pendingAdds).toEqual(["/music/new"]);
  });
});
