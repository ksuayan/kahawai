import { describe, expect, it } from "vitest";
import { tauri } from "@pw/test/tauri-mock";
import { mountApp, openSelect, options, pick, settle } from "../test/helpers";
import { dialog } from "../test/dialog-mock";
import SettingsTab from "./SettingsTab.vue";
import { useSetupStore } from "../stores/setup";
import { useEnrichmentStore } from "../stores/enrichment";

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

describe("SettingsTab: album info", () => {
  const status = (over: Record<string, unknown> = {}) => ({
    enabled: false,
    min_confidence: 0.9,
    coverage: { total_albums: 5793, with_embedded_mbid: 1204, matched_online: 0, no_match: 0, pending_lookup: 4589 },
    job: null,
    ...over,
  });
  const button = (w: ReturnType<typeof boot>["wrapper"], text: string) =>
    w.findAll("button").find((b) => b.text() === text);

  it("is off by default, shows coverage, and Look up now waits for it to be turned on", async () => {
    inTauri();
    tauri.on("setup_enrichment_status", status());
    const { wrapper } = boot();
    await settle();
    const box = wrapper.find('input[type="checkbox"]').element as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(wrapper.text()).toContain("5,793 albums");
    expect(wrapper.text()).toContain("4,589 waiting");
    expect((button(wrapper, "Look up now")!.element as HTMLButtonElement).disabled).toBe(true);
  });

  it("turning it on sends the current threshold; changing strictness keeps it on", async () => {
    inTauri();
    tauri.on("setup_enrichment_status", status()).on("setup_set_enrichment", undefined);
    const { wrapper } = boot();
    await settle();
    await wrapper.find('input[type="checkbox"]').setValue(true);
    await settle();
    expect(tauri.callsTo("setup_set_enrichment")).toEqual([{ enabled: true, minConfidence: 0.9 }]);

    tauri.on("setup_enrichment_status", status({ enabled: true }));
    await useEnrichmentStore().load();
    await settle();
    const trigger = wrapper.get('[role="combobox"]');
    expect(trigger.text()).toBe("Balanced (90%)");
    await openSelect(trigger.element as HTMLElement);
    expect(options().map((o) => o.textContent?.trim())).toEqual([
      "Relaxed (80%)",
      "Balanced (90%)",
      "Strict (95%)",
    ]);
    pick(options().find((o) => o.textContent?.includes("Strict"))!);
    await settle();
    expect(tauri.callsTo("setup_set_enrichment")[1]).toEqual({ enabled: true, minConfidence: 0.95 });
  });

  it("a lookup paused because MusicBrainz is unreachable says so and offers Resume and Cancel", async () => {
    inTauri();
    const message = "Paused: can't reach MusicBrainz. Nothing was lost; 4589 albums still to look up.";
    tauri
      .on("setup_enrichment_status", status({ enabled: true, job: { id: "job-0009", kind: "enrich_metadata", label: "Album info lookup", progress: 0.1, status: "paused", message } }))
      .on("setup_enrichment_action", undefined);
    const { wrapper } = boot();
    await settle();
    expect(wrapper.text()).toContain("Nothing was lost");
    expect(button(wrapper, "Look up now")).toBeUndefined();
    expect(button(wrapper, "Pause")).toBeUndefined();
    await button(wrapper, "Resume")!.trigger("click");
    await settle();
    expect(tauri.callsTo("setup_enrichment_action")).toEqual([{ action: "resume", jobId: "job-0009" }]);
    expect(button(wrapper, "Cancel")).toBeDefined();
  });

  it("with nothing waiting but albums not found, offers to retry them", async () => {
    inTauri();
    tauri
      .on("setup_enrichment_status", status({ enabled: true, coverage: { total_albums: 5754, with_embedded_mbid: 1873, matched_online: 2900, no_match: 981, pending_lookup: 0 } }))
      .on("setup_enrichment_action", undefined);
    const { wrapper } = boot();
    await settle();
    expect(button(wrapper, "Look up now")).toBeUndefined();
    await button(wrapper, "Retry not found (981)")!.trigger("click");
    await settle();
    expect(tauri.callsTo("setup_enrichment_action")).toEqual([{ action: "retry", jobId: null }]);
  });
});

describe("SettingsTab: music folders", () => {
  it("reads the running server's folders when it opens with none (the server was still starting)", async () => {
    inTauri();
    tauri
      .on("setup_enrichment_status", undefined)
      .on("setup_get_running_config", { music_dirs: ["/Volumes/NetMusic"], bind: "0.0.0.0:8080", db_path: "/d/music.db", preferred_ladder: [], dsd_story: "pcm", scan_on_startup: true })
      .on("setup_validate_dir", okValidation);
    const { wrapper } = boot();
    await settle();
    expect(wrapper.text()).toContain("/Volumes/NetMusic");
  });
});

describe("SettingsTab: logs", () => {
  it("reveals the log folder in Finder", async () => {
    const { wrapper } = boot();
    await wrapper.get('[data-testid="reveal-logs"]').trigger("click");
    expect(tauri.callsTo("setup_reveal_logs")).toHaveLength(1);
  });
});


describe("SettingsTab: audiobook folders", () => {
  const books = { exists: true, is_dir: true, readable: true, writable: true, audio_files: 40, audiobooks: 12, truncated: false };

  it("is a separate list from the music folders, shows how many audiobooks each holds, and queues a removal", async () => {
    tauri.on("setup_audiobook_folders", [{ id: 4, path: "/srv/books", name: "books" }]).on("setup_validate_audiobook_dir", books);
    const { wrapper } = boot();
    const setup = useSetupStore();
    setup.runningDirs = [{ path: "/music/a", validating: false, validation: okValidation }];
    await settle();
    const list = wrapper.get('[data-testid="audiobook-roots"]');
    expect(list.text()).toContain("/srv/books");
    expect(list.text()).not.toContain("/music/a");
    expect(wrapper.get('[data-testid="audiobook-chip"]').text()).toBe("12 audiobooks");
    const apply = () => wrapper.get('[data-testid="apply-audiobooks"]').element as HTMLButtonElement;
    expect(apply().disabled).toBe(true);
    await wrapper.get('[data-testid="remove-audiobook-folder"]').trigger("click");
    await settle();
    expect(setup.pendingBookRemoves).toEqual([4]);
    expect(tauri.callsTo("setup_apply_audiobooks")).toHaveLength(0);
    expect(apply().disabled).toBe(false);
    expect(wrapper.text()).toContain("0 to add, 1 to remove");
  });

  it("a picked folder appears marked new with its count, and Apply sends it", async () => {
    inTauri();
    tauri.on("setup_audiobook_folders", []).on("setup_validate_audiobook_dir", { ...books, audiobooks: 1 }).on("setup_apply_audiobooks", undefined).on("setup_recent_scans", []);
    const { wrapper } = boot();
    await settle();
    dialog.nextPath = "/srv/shelf";
    await wrapper.get('[data-testid="add-audiobook-folder"]').trigger("click");
    await settle();
    const row = wrapper.get('[data-testid="audiobook-row"]');
    expect(row.text()).toContain("/srv/shelf");
    expect(row.text()).toContain("new");
    expect(wrapper.get('[data-testid="audiobook-chip"]').text()).toBe("1 audiobook");
    await wrapper.get('[data-testid="apply-audiobooks"]').trigger("click");
    await settle();
    expect(tauri.callsTo("setup_apply_audiobooks")).toEqual([{ input: { add: ["/srv/shelf"], remove: [] } }]);
  });

  it("says when a folder holds no audiobooks, and shows the server's reason when Apply is refused", async () => {
    inTauri();
    tauri.on("setup_audiobook_folders", []).on("setup_validate_audiobook_dir", { ...books, audiobooks: 0 }).on("setup_recent_scans", []);
    const { wrapper } = boot();
    await settle();
    dialog.nextPath = "/srv/empty";
    await wrapper.get('[data-testid="add-audiobook-folder"]').trigger("click");
    await settle();
    expect(wrapper.get('[data-testid="audiobook-chip"]').text()).toBe("no audiobooks found");
    tauri.on("setup_apply_audiobooks", () => {
      throw "/srv/empty is not a folder";
    });
    await wrapper.get('[data-testid="apply-audiobooks"]').trigger("click");
    await settle();
    expect(wrapper.text()).toContain("/srv/empty is not a folder");
  });
});

describe("SettingsTab: online sources", () => {
  it("is off by default and turning it on tells the server", async () => {
    inTauri();
    tauri.on("setup_online_sources", false).on("setup_set_online_sources", undefined);
    const { wrapper } = boot();
    await settle();
    const box = wrapper.get('[data-testid="online-sources"]');
    expect((box.element as HTMLInputElement).checked).toBe(false);
    tauri.on("setup_online_sources", true);
    await box.setValue(true);
    await settle();
    expect(tauri.callsTo("setup_set_online_sources")).toEqual([{ enabled: true }]);
    expect((wrapper.get('[data-testid="online-sources"]').element as HTMLInputElement).checked).toBe(true);
  });

  it("shows the reason when the server refuses", async () => {
    inTauri();
    tauri.on("setup_online_sources", false).on("setup_set_online_sources", () => {
      throw new Error("could not save the config");
    });
    const { wrapper } = boot();
    await settle();
    await wrapper.get('[data-testid="online-sources"]').setValue(true);
    await settle();
    expect(wrapper.text()).toContain("could not save the config");
  });
});
