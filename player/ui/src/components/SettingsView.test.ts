import { describe, expect, it, vi } from "vitest";
import { makeAlbum, mockFetch } from "../test/fixtures";
import { $$, mountApp, openSelect, options, pick, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useDspStore } from "../stores/dsp";
import { useJobsStore } from "../stores/jobs";
import { useSettingsStore } from "../stores/settings";
import { useToastsStore } from "../stores/toasts";
import SettingsView from "./SettingsView.vue";

const devices = [
  { name: "BenQ PD3225U", is_default: false },
  { name: "Built-in Output", is_default: true },
  { name: "FIIO K15 ", is_default: false },
  { name: "RØDE Connect System", is_default: false },
];

function bootTauri(chosen: string | null = null) {
  tauri
    .on("get_dsp_settings", { eq_bands: [{ band_type: "peaking", freq: 1000, gain_db: 3, q: 1 }], eq_enabled: true, loudness_enabled: false, loudness_target: -14 })
    .on("get_output_devices", devices)
    .on("get_output_device", chosen)
    .on("dop_status", { supported_rates: [176400, 352800], exclusive_available: true });
}

async function mountSettings(chosen: string | null = null) {
  bootTauri(chosen);
  const { wrapper } = mountApp(SettingsView);
  await useDspStore().init();
  await settle();
  return wrapper;
}
const select = (w: Awaited<ReturnType<typeof mountSettings>>, label: string) =>
  w.get(`[role="combobox"][aria-label="${label}"]`).element as HTMLElement;
const optionLabels = () => options().map((o) => o.textContent?.trim());
const button = (w: Awaited<ReturnType<typeof mountSettings>>, text: string | RegExp) =>
  w.findAll("button").find((b) => (typeof text === "string" ? b.text() === text : text.test(b.text())))!;

describe("Settings: output device", () => {
  it("lists the system default plus every device, with the default's name", async () => {
    const w = await mountSettings();
    expect(select(w, "Output device").textContent).toContain("System default (Built-in Output)");
    await openSelect(select(w, "Output device"));
    expect(optionLabels()).toEqual(["System default (Built-in Output)", "BenQ PD3225U", "Built-in Output", "FIIO K15", "RØDE Connect System"]);
  });

  it("shows the saved device as selected", async () => {
    const w = await mountSettings("RØDE Connect System");
    expect(select(w, "Output device").textContent).toContain("RØDE Connect System");
  });

  it("switches the output by exact name, including a trailing space", async () => {
    const w = await mountSettings();
    await openSelect(select(w, "Output device"));
    pick(options()[3]); // "FIIO K15 "
    await settle();
    expect(tauri.callsTo("set_output_device")).toEqual([{ name: "FIIO K15 " }]);
    expect(useDspStore().outputDevice).toBe("FIIO K15 ");
  });

  it("goes back to the system default with null", async () => {
    const w = await mountSettings("BenQ PD3225U");
    await openSelect(select(w, "Output device"));
    pick(options()[0]);
    await settle();
    expect(tauri.callsTo("set_output_device")).toEqual([{ name: null }]);
    expect(useDspStore().outputDevice).toBeNull();
  });

  it("warns when the saved device is unplugged, and keeps it selectable", async () => {
    const w = await mountSettings("Old USB DAC");
    expect(w.get('[data-testid="device-missing"]').text()).toContain("Old USB DAC");
    expect(w.get('[data-testid="device-missing"]').text()).toContain("not connected");
    expect(select(w, "Output device").textContent).toContain("Old USB DAC — not connected");
  });

  it("has no warning when the saved device is present", async () => {
    const w = await mountSettings("BenQ PD3225U");
    expect(w.find('[data-testid="device-missing"]').exists()).toBe(false);
  });

  it("rescans devices from the refresh button and clears the warning when the DAC returns", async () => {
    const w = await mountSettings("Old USB DAC");
    tauri.on("get_output_devices", [...devices, { name: "Old USB DAC", is_default: false }]);
    await w.get('button[aria-label="Rescan output devices"]').trigger("click");
    await settle();
    expect(w.find('[data-testid="device-missing"]').exists()).toBe(false);
  });

  it("reports DoP capability of the chosen device", async () => {
    const w = await mountSettings();
    expect(w.text()).toContain("Exclusive DoP is available on this Mac.");
    expect(w.text()).toContain("176.4 kHz, 352.8 kHz");
    tauri.on("dop_status", { supported_rates: [], exclusive_available: true });
    await useDspStore().refreshDevices();
    await settle();
    expect(w.text()).toContain("reports no DoP-capable rate");
  });

  it("says so when there are no devices", async () => {
    tauri.on("get_output_devices", []);
    tauri.on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14 });
    const { wrapper } = mountApp(SettingsView);
    await useDspStore().init();
    await settle();
    expect(wrapper.text()).toContain("No output devices reported.");
  });
});

describe("Settings: playback preferences", () => {
  it("sets the default stream format (and can return to Auto)", async () => {
    const w = await mountSettings();
    await openSelect(select(w, "Default stream format"));
    expect(optionLabels()).toEqual(["Auto (server default)", "Passthrough (original)", "FLAC", "OPUS", "MP3", "DOP"]);
    pick(options()[2]);
    await settle();
    expect(tauri.callsTo("set_format")).toEqual([{ fmt: "flac" }]);
    expect(useSettingsStore().globalFormat).toBe("flac");

    await openSelect(select(w, "Default stream format"));
    pick(options()[0]);
    await settle();
    expect(tauri.callsTo("set_format").at(-1)).toEqual({ fmt: null });
  });

  it("sets DSD handling", async () => {
    const w = await mountSettings();
    expect(select(w, "DSD handling").textContent).toContain("Convert to PCM");
    await openSelect(select(w, "DSD handling"));
    pick(options()[1]);
    await settle();
    expect(tauri.callsTo("set_dsd_story")).toEqual([{ story: "native" }]);
    expect(select(w, "DSD handling").textContent).toContain("Native DoP");
  });
});

describe("Settings: server URL", () => {
  it("saves the URL, checks the connection and reloads the library", async () => {
    const calls = mockFetch({
      "/api/health": { status: "ok" },
      "/api/albums": { items: [makeAlbum()], page: 1, per_page: 500, total: 1 },
      "/api/artists": [],
      "/api/playlists": [],
    });
    const w = await mountSettings();
    await typeInto(w.get('input[aria-label="Server URL"]').element as HTMLInputElement, "http://nas:8080/");
    await button(w, "Save").trigger("click");
    await settle();
    expect(tauri.callsTo("set_server_url")).toEqual([{ url: "http://nas:8080" }]);
    expect(w.get('[data-testid="connection-status"]').text()).toContain("Server reachable");
    expect(calls.some((c) => c.url.startsWith("http://nas:8080/api/albums"))).toBe(true);
  });

  it("reports an unreachable server", async () => {
    mockFetch({});
    const w = await mountSettings();
    await button(w, "Save").trigger("click");
    await settle();
    expect(w.get('[data-testid="connection-status"]').text()).toContain("Server unreachable");
  });

  it("rejects an empty URL without saving", async () => {
    const w = await mountSettings();
    await typeInto(w.get('input[aria-label="Server URL"]').element as HTMLInputElement, "   ");
    await button(w, "Save").trigger("click");
    await settle();
    expect(w.text()).toContain("URL cannot be empty.");
    expect(tauri.callsTo("set_server_url")).toHaveLength(0);
  });

  it("checks the connection on demand", async () => {
    mockFetch({ "/api/health": { status: "ok" } });
    const w = await mountSettings();
    expect(w.get('[data-testid="connection-status"]').text()).toContain("not checked");
    await w.get('button[aria-label="Check connection"]').trigger("click");
    await settle();
    expect(w.get('[data-testid="connection-status"]').text()).toContain("Server reachable");
  });
});

describe("Settings: EQ and loudness", () => {
  const sw = (w: Awaited<ReturnType<typeof mountSettings>>) => w.findAll('[role="switch"]');

  it("toggles EQ through the core", async () => {
    const w = await mountSettings();
    const eq = sw(w)[0];
    expect(eq.attributes("aria-checked")).toBe("true");
    await eq.trigger("click");
    await settle();
    expect(tauri.callsTo("set_eq_enabled")).toEqual([{ enabled: false }]);
    expect(useDspStore().eqEnabled).toBe(false);
  });

  it("lists the EQ bands with their values and can add and remove", async () => {
    const w = await mountSettings();
    expect(w.findAll('[data-testid="eq-band"]')).toHaveLength(1);
    await button(w, /Add band/).trigger("click");
    await settle();
    expect(w.findAll('[data-testid="eq-band"]')).toHaveLength(2);
    await w.findAll('button[aria-label="Remove band"]')[0].trigger("click");
    await settle();
    expect(w.findAll('[data-testid="eq-band"]')).toHaveLength(1);
  });

  it("disables Add band at the eight-band limit", async () => {
    const w = await mountSettings();
    for (let i = 0; i < 10; i++) useDspStore().addBand();
    await settle();
    expect(useDspStore().rows.length).toBe(8);
    expect(button(w, /Add band/).attributes("disabled")).toBeDefined();
  });

  it("edits a band's frequency and applies it live", async () => {
    const w = await mountSettings();
    const freq = w.get('[data-testid="eq-band"] input[type="number"]').element as HTMLInputElement;
    freq.value = "2500";
    freq.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const last = tauri.callsTo("set_eq_bands").at(-1) as { bands: { freq: number }[] };
    expect(last.bands[0].freq).toBe(2500);
  });

  it("rejects an out-of-range band value with a message", async () => {
    const w = await mountSettings();
    const freq = w.get('[data-testid="eq-band"] input[type="number"]').element as HTMLInputElement;
    freq.value = "5";
    freq.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(w.text()).toContain("Frequency must be 10–24000 Hz.");
  });

  it("toggles loudness normalisation and validates the target", async () => {
    const w = await mountSettings();
    await sw(w).at(-1)!.trigger("click");
    await settle();
    expect(tauri.callsTo("set_loudness_enabled")).toEqual([{ enabled: true }]);

    const target = w.findAll('input[type="number"]').at(-1)!.element as HTMLInputElement;
    await typeInto(target, "-23"); // input event updates the model, change commits it
    target.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(tauri.callsTo("set_loudness_target")).toEqual([{ lufs: -23 }]);

    await typeInto(target, "5");
    target.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(w.text()).toContain("Target must be −40…−1 LUFS.");
  });
});

describe("Settings: library scan", () => {
  it("starts a scan and shows progress", async () => {
    const job = { id: "job-1", kind: "scan", label: "scan", status: "running", progress: 0.4, message: null, payload: null };
    const calls = mockFetch({
      "/api/scan": () => new Response(JSON.stringify(job), { status: 202 }),
      "/api/jobs": () => new Response(JSON.stringify([job]), { status: 200 }),
    });
    const w = await mountSettings();
    await button(w, "Scan library").trigger("click");
    await settle();
    expect(calls.some((c) => c.url.endsWith("/api/scan") && c.init?.method === "POST")).toBe(true);
    expect(useToastsStore().toasts.some((t) => t.kind === "progress")).toBe(true);
    useJobsStore().jobs = [job] as never;
    await settle();
  });

  it("treats 'already running' as information, not a failure", async () => {
    mockFetch({ "/api/scan": () => new Response(JSON.stringify({ error: "scan already running" }), { status: 409 }), "/api/jobs": () => new Response("[]", { status: 200 }) });
    const w = await mountSettings();
    await button(w, "Scan library").trigger("click");
    await settle();
    expect(useToastsStore().toasts.at(-1)).toMatchObject({ kind: "info", title: "Scan already running" });
  });

  it("lists recent jobs with their status", async () => {
    const w = await mountSettings();
    useJobsStore().jobs = [
      { id: "a", kind: "scan", label: "s", status: "done", progress: 1, message: null, payload: null },
      { id: "b", kind: "scan", label: "s", status: "failed", progress: 1, message: "x", payload: null },
    ] as never;
    await settle();
    const rows = w.findAll('[data-testid="recent-job"]');
    expect(rows.map((r) => r.text().replace(/\s+/g, ""))).toEqual(["Libraryscandone", "Libraryscanfailed"]);
  });
});

describe("Settings: album-art cache", () => {
  it("is hidden outside the Tauri app", async () => {
    const w = await mountSettings();
    expect(w.text()).not.toContain("Album art cache");
  });

  it("shows the cache size and clears it", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    let stats = { bytes: 5 * 1024 * 1024, files: 42 };
    tauri.on("artwork_cache_stats", () => stats).on("clear_artwork_cache", () => {
      stats = { bytes: 0, files: 0 };
      return 42;
    });
    mockFetch({ "/api/health": { status: "ok" }, "/api/albums": { items: [], page: 1, per_page: 500, total: 0 }, "/api/artists": [] });
    const w = await mountSettings();
    expect(w.text()).toContain("42 images · 5.0 MB");
    await button(w, "Clear cache").trigger("click");
    await settle();
    expect(tauri.callsTo("clear_artwork_cache")).toHaveLength(1);
    expect(w.text()).toContain("0 images · 1 KB");
    expect(button(w, "Clear cache").attributes("disabled")).toBeDefined();
  });
});

describe("Settings: shortcuts reference", () => {
  it("documents the global keys", async () => {
    const w = await mountSettings();
    const keys = $$("kbd").map((k) => k.textContent);
    expect(keys).toEqual(expect.arrayContaining(["Space", "← / →", "↑ / ↓", "N / P", "F", "1 … 6"]));
    void vi;
    expect(w.text()).toContain("Available everywhere except while typing");
  });
});
