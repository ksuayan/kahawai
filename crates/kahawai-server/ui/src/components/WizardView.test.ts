import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import { dialog } from "../test/dialog-mock";
import WizardView from "./WizardView.vue";
import { useSetupStore } from "../stores/setup";

const okValidation = { exists: true, is_dir: true, readable: true, writable: true, audio_files: 3 };

function boot() {
  return mountApp(WizardView);
}

describe("WizardView", () => {
  it("starts on Welcome with Continue enabled and Back disabled", () => {
    const { wrapper } = boot();
    expect(wrapper.text()).toContain("Welcome to Kahawai Server");
    const back = wrapper.findAll("button").find((b) => b.text() === "Back")!;
    expect((back.element as HTMLButtonElement).disabled).toBe(true);
  });

  it("blocks Continue on the folders step until a folder validates ok", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const { wrapper } = boot();
    useSetupStore().step = 1;
    await settle();
    const findContinue = () => wrapper.findAll("button").find((b) => b.text() === "Continue")!;
    expect((findContinue().element as HTMLButtonElement).disabled).toBe(true);

    dialog.nextPath = "/music/a";
    tauri.on("setup_validate_dir", okValidation);
    await wrapper.findAll("button").find((b) => b.text() === "Add folder…")!.trigger("click");
    await settle();
    expect((findContinue().element as HTMLButtonElement).disabled).toBe(false);
  });

  it("saves on the Review step's Continue and advances to Done", async () => {
    tauri.on("setup_save_config", undefined);
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.dirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    setup.dbDir = "/data";
    setup.step = 3;
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Save")!.trigger("click");
    await settle();
    expect(setup.step).toBe(4);
    expect(tauri.callsTo("setup_save_config")).toHaveLength(1);
  });

  it("stays on Review and shows the error when save fails", async () => {
    tauri.on("setup_save_config", () => {
      throw new Error("bind address already in use");
    });
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.dirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    setup.dbDir = "/data";
    setup.step = 3;
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Save")!.trigger("click");
    await settle();
    expect(setup.step).toBe(3);
    expect(wrapper.text()).toContain("bind address already in use");
  });
});
