import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, settle } from "../test/helpers";
import { dialog } from "../test/dialog-mock";
import WizardView from "./WizardView.vue";
import { useSetupStore } from "../stores/setup";

const okValidation = { exists: true, is_dir: true, readable: true, writable: true, audio_files: 3, truncated: false };

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
    setup.step = 4;
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Save")!.trigger("click");
    await settle();
    expect(setup.step).toBe(5);
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
    setup.step = 4;
    await settle();
    await wrapper.findAll("button").find((b) => b.text() === "Save")!.trigger("click");
    await settle();
    expect(setup.step).toBe(4);
    expect(wrapper.text()).toContain("bind address already in use");
  });
});

describe("WizardView: audiobook folders step", () => {
  it("sits after the music folders, is optional, and does not gate Continue", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.step = 2;
    await settle();
    expect(wrapper.text()).toContain("Audiobook folders");
    expect(wrapper.text()).toContain("Optional");
    const cont = wrapper.findAll("button").find((b) => b.text() === "Continue")!;
    expect((cont.element as HTMLButtonElement).disabled).toBe(false);
    await cont.trigger("click");
    expect(setup.step).toBe(3);
    expect(wrapper.text()).toContain("Database");
  });

  it("lists the folders added and shows them in the review", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.on("setup_validate_audiobook_dir", { ...okValidation, audio_files: 30, audiobooks: 5 });
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.step = 2;
    await settle();
    dialog.nextPath = "/books/shelf";
    await wrapper.get('[data-testid="add-audiobook-folder"]').trigger("click");
    await settle();
    expect(wrapper.findAll('[data-testid="audiobook-dir"]')).toHaveLength(1);
    expect(wrapper.get('[data-testid="audiobook-chip"]').text()).toBe("5 audiobooks");
    setup.dirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    setup.step = 4;
    await settle();
    expect(wrapper.get('[data-testid="review-audiobooks"]').text()).toContain("/books/shelf");
  });

  it("Review says None when there are no audiobook folders", async () => {
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.dirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    setup.step = 4;
    await settle();
    expect(wrapper.get('[data-testid="review-audiobooks"]').text()).toBe("None");
  });
});
