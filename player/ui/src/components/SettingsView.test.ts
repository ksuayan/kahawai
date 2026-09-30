import { describe, expect, it, vi } from "vitest";
import { EQ_LIMITS } from "../eqResponse";
import { makeAlbum, makeState, makeTrack, mockFetch } from "../test/fixtures";
import { $$, mountApp, openSelect, options, pick, settle, typeInto } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useAnalogStore } from "../stores/analog";
import { useDspStore } from "../stores/dsp";
import { useJobsStore } from "../stores/jobs";
import { usePlayerStore } from "../stores/player";
import { useSettingsStore } from "../stores/settings";
import { useToastsStore } from "../stores/toasts";
import SettingsView from "./SettingsView.vue";

const devices = [
  { name: "BenQ PD3225U", is_default: false },
  { name: "Built-in Output", is_default: true },
  { name: "FIIO K15 ", is_default: false },
  { name: "RØDE Connect System", is_default: false },
];

const DEFAULT_DOP = { supported_rates: [176400, 352800], exclusive_available: true };

function bootTauri(chosen: string | null = null, dop: Record<string, unknown> = DEFAULT_DOP) {
  tauri
    .on("get_dsp_settings", { eq_bands: [{ band_type: "peaking", freq: 1000, gain_db: 3, q: 1 }], eq_enabled: true, loudness_enabled: false, loudness_target: -14 })
    .on("get_output_devices", devices)
    .on("get_output_device", chosen)
    .on("dop_status", dop);
}

async function mountSettings(chosen: string | null = null, dop?: Record<string, unknown>) {
  bootTauri(chosen, dop);
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
  it("explains the format setting in plain words and points to the per-track picker", async () => {
    const w = await mountSettings();
    const section = w.findAll("section").find((x) => x.find("h3").text() === "Stream format")!;
    expect(section.exists()).toBe(true);
    expect(section.text()).toContain("Auto is best for most people");
    expect(section.text()).toContain("every track");
    expect(section.text()).toContain("player bar");
    expect(section.text()).not.toMatch(/set_format/); // no internal command names
  });

  it("defaults to Auto (recommended)", async () => {
    const w = await mountSettings();
    expect(select(w, "Default stream format").textContent).toContain("Auto (recommended)");
  });

  it("sets the default stream format (and can return to Auto)", async () => {
    const w = await mountSettings();
    await openSelect(select(w, "Default stream format"));
    expect(optionLabels()).toEqual(["Auto (recommended)", "Passthrough (original)", "FLAC", "OPUS", "MP3", "DOP"]);
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
    expect(select(w, "DSD handling").textContent).toContain("Auto");
    await openSelect(select(w, "DSD handling"));
    pick(options()[2]);
    await settle();
    expect(tauri.callsTo("set_dsd_story")).toEqual([{ story: "native" }]);
    expect(select(w, "DSD handling").textContent).toContain("Native DoP");
  });

  it("says what Auto does for a known DSD DAC and for an unknown one", async () => {
    const known = await mountSettings(null, {
      supported_rates: [176400],
      exclusive_available: true,
      device: "FIIO K15 ",
      known_dsd_device: true,
      user_confirmed: false,
      auto_resolves_to: "native",
      capabilities: {
        name: "FIIO K15 ",
        transport: "usb",
        external_dac: true,
        sample_rates: [44100, 48000, 88200, 96000, 176400],
        bit_depths: [16, 32],
        float32: false,
        dop_rates: [176400],
        exclusive_available: true,
      },
    });
    const note = known.get('[data-testid="dsd-auto-note"]').text();
    expect(note).toContain("FIIO K15");
    expect(note).toContain("known DSD DAC");
    expect(known.get('[data-testid="dsd-rate-probe"]').text()).toContain("DSD64 ✓");
    expect(known.get('[data-testid="dsd-rate-probe"]').text()).toContain("DSD128 ✗");

    const unknown = await mountSettings(null, {
      supported_rates: [176400],
      exclusive_available: true,
      device: "Some USB DAC",
      known_dsd_device: false,
      user_confirmed: false,
      auto_resolves_to: "convert",
    });
    expect(unknown.get('[data-testid="dsd-auto-note"]').text()).toContain("converted to FLAC");
  });

  it("lets the user confirm an unlisted DAC decodes DoP", async () => {
    const w = await mountSettings(null, {
      supported_rates: [176400],
      exclusive_available: true,
      device: "Some USB DAC",
      known_dsd_device: false,
      user_confirmed: false,
      auto_resolves_to: "convert",
    });
    await w.get('[data-testid="dsd-device-confirm"] [role="switch"]').trigger("click");
    await settle();
    expect(tauri.callsTo("set_dsd_device_confirmed")).toEqual([{ confirmed: true }]);
  });

  it("warns that a global stream format does not apply to natively-played DSD", async () => {
    const w = await mountSettings();
    useSettingsStore().dsdStory = "native";
    useSettingsStore().globalFormat = "passthrough";
    await settle();
    expect(w.find('[data-testid="dsd-format-ignored"]').exists()).toBe(true);
  });
});

describe("Settings: bit-perfect output", () => {
  const section = (w: Awaited<ReturnType<typeof mountSettings>>) => w.findAll("section").find((x) => x.find("h3").text() === "Bit-perfect output")!;

  it("explains what it does in plain words, including the MQA use", async () => {
    const w = await mountSettings();
    const t = section(w).text();
    expect(t).toContain("untouched");
    expect(t).toContain("file's own");
    expect(t).toContain("MQA");
    expect(t).toContain("EQ, loudness, volume and format conversion are bypassed");
  });

  it("follows the quality mode by default and offers Auto / Off / MQA files only / All tracks", async () => {
    const w = await mountSettings();
    expect(select(w, "Bit-perfect output").textContent).toContain("Auto");
    await openSelect(select(w, "Bit-perfect output"));
    expect(optionLabels()).toEqual([
      "Auto (follows Sound quality)",
      "Off (shared output; EQ and volume work)",
      "MQA files only",
      "All tracks",
    ]);
  });

  it("saves the chosen mode through the core", async () => {
    const w = await mountSettings();
    await openSelect(select(w, "Bit-perfect output"));
    pick(options()[2]);
    await settle();
    expect(tauri.callsTo("set_bit_perfect")).toEqual([{ mode: "mqa" }]);
    expect(useSettingsStore().bitPerfect).toBe("mqa");
    expect(select(w, "Bit-perfect output").textContent).toContain("MQA files only");

    await openSelect(select(w, "Bit-perfect output"));
    pick(options()[3]);
    await settle();
    expect(tauri.callsTo("set_bit_perfect").at(-1)).toEqual({ mode: "all" });
  });

  it("warns about the trade-offs only while it is on", async () => {
    const w = await mountSettings();
    expect(w.find('[data-testid="bit-perfect-notes"]').exists()).toBe(false);
    useSettingsStore().bitPerfect = "mqa";
    await settle();
    const notes = w.get('[data-testid="bit-perfect-notes"]').text();
    expect(notes).toContain("DAC's own volume control");
    expect(notes).toContain("Other apps can't play");
    expect(notes).toContain("one at a time");
    expect(notes).toContain("plays normally instead");
    useSettingsStore().bitPerfect = "auto";
    await settle();
    expect(w.find('[data-testid="bit-perfect-notes"]').exists()).toBe(false);
  });

  it("says it is macOS-only where the exclusive path does not exist", async () => {
    tauri
      .on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14 })
      .on("get_output_devices", devices)
      .on("get_output_device", null)
      .on("dop_status", { supported_rates: [], exclusive_available: false });
    const { wrapper } = mountApp(SettingsView);
    await useDspStore().init();
    await settle();
    const sec = wrapper.findAll("section").find((x) => x.find("h3").text() === "Bit-perfect output")!;
    expect(sec.text()).toContain("macOS-only");
    expect(sec.find('[role="combobox"]').exists()).toBe(false);
  });
});

describe("Settings: Advanced", () => {
  it("stays editable when Best quality is on, so it carries no such note", async () => {
    const w = await mountSettings();
    expect(w.get('[data-testid="advanced"]').text()).not.toContain("Disabled when");
  });
});

describe("Settings: Experimental", () => {
  it("is collapsed by default and holds Analog warmth", async () => {
    const w = await mountSettings();
    const box = w.get('[data-testid="experimental"]');
    expect(box.attributes("open")).toBeUndefined();
    expect(box.find("summary").text()).toContain("Experimental");
    expect(box.text()).toContain("Analog warmth");
    // Not a top-level section any more (mentions of it elsewhere, e.g. the
    // keyboard-shortcuts list, don't count).
    const top = w.findAll(":scope > section").filter((s) => s.find("h3").text() === "Analog warmth");
    expect(top).toHaveLength(0);
  });

  it("says in the collapsed header when analog warmth is on, so an active effect is never hidden", async () => {
    const w = await mountSettings();
    expect(w.find('[data-testid="experimental-active"]').exists()).toBe(false);
    useAnalogStore().a = { ...useAnalogStore().a, enabled: true };
    useAnalogStore().active = "a";
    await settle();
    expect(w.get('[data-testid="experimental-active"]').text()).toContain("Analog warmth on");
    expect(w.get('[data-testid="experimental"]').attributes("open")).toBeUndefined();
  });
});

describe("Settings: section order", () => {
  it("follows the agreed sequence", async () => {
    const w = await mountSettings();
    const titles = w
      .findAll('[data-testid="signal-path"] h3, section > h3, summary, h3')
      .map((e) => e.text().replace(/[●]/g, "").replace(/\s+/g, " ").trim());
    const wanted = [
      "Media file",
      "Output device",
      "Server",
      "Audio output",
      "Sound quality",
      "Crossfeed",
      "Parametric EQ",
      "Loudness normalization",
      "Limiter",
      "Advanced",
      "Experimental",
      "Analog warmth",
      "Library",
      "Album art cache",
    ];
    const seen = wanted.map((t) => titles.findIndex((x) => x.startsWith(t)));
    expect(seen.filter((i) => i < 0), "every section is present (album art needs Tauri)").toEqual(
      seen.filter((i, n) => i < 0 && wanted[n] === "Album art cache"),
    );
    const present = seen.filter((i) => i >= 0);
    expect(present).toEqual([...present].sort((a, b) => a - b));
    // Listening suggestions is part of the Analog section, so it follows it.
    const analog = titles.findIndex((x) => x.startsWith("Analog warmth"));
    const library = titles.findIndex((x) => x.startsWith("Library"));
    expect(analog).toBeLessThan(library);
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
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("offline"); // nothing answers
    });
    const w = await mountSettings();
    await button(w, "Save").trigger("click");
    await settle();
    expect(w.get('[data-testid="connection-status"]').text()).toContain("Server unreachable");
  });

  it("names the Kahawai server it reaches: version and build", async () => {
    mockFetch({
      "/api/identity": {
        service: "kahawai-server",
        name: "Kahawai Server",
        version: "0.1.0",
        api_version: 1,
        build: { commit: "cd9b827", dirty: false, built_at: "2026-09-30T19:02:11Z", profile: "release", target: "aarch64-apple-darwin" },
        catalog_id: "3f1c",
        started_at: 1_790_794_931_000,
      },
    });
    const w = await mountSettings();
    const text = w.get('[data-testid="connection-text"]');
    expect(text.text()).toBe("Server reachable · Kahawai Server 0.1.0 · build cd9b827 (release, aarch64-apple-darwin)");
    expect(text.attributes("title")).toContain("2026-09-30T19:02:11Z");
    expect(w.get('[data-testid="server-light"]').attributes("data-state")).toBe("connected");
    expect(w.find('[data-testid="server-source"]').exists()).toBe(false); // an older server: no source_url
  });

  it("shows where the server's source code is (its AGPL source offer)", async () => {
    mockFetch({
      "/api/identity": {
        service: "kahawai-server",
        name: "Kahawai Server",
        version: "0.1.0",
        api_version: 1,
        build: { commit: "cd9b827", dirty: false, built_at: "2026-09-30T19:02:11Z", profile: "release", target: "aarch64-apple-darwin" },
        catalog_id: "3f1c",
        started_at: 1_790_794_931_000,
        source_url: "https://github.com/ksuayan/kahawai",
      },
    });
    const w = await mountSettings();
    expect(w.get('[data-testid="server-source"]').text()).toContain("Source code: https://github.com/ksuayan/kahawai");
  });

  it("tells a Kahawai server from something else on the same address", async () => {
    mockFetch({ "/api/identity": { service: "some-other-app" } });
    const w = await mountSettings();
    expect(w.get('[data-testid="connection-text"]').text()).toBe(
      "Something answers at this address, but it isn't a Kahawai server",
    );
    expect(w.get('[data-testid="server-light"]').attributes("data-state")).toBe("offline");
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
    const w = await mountSettings(); // fetch is offline in tests, so the on-open check fails
    expect(w.get('[data-testid="connection-status"]').text()).toContain("Server unreachable");
    mockFetch({ "/api/health": { status: "ok" } });
    await w.get('button[aria-label="Check connection"]').trigger("click");
    await settle();
    expect(w.get('[data-testid="connection-status"]').text()).toContain("Server reachable");
  });

  it("shows a green light next to the Server title when connected and a red one when offline", async () => {
    mockFetch({ "/api/health": { status: "ok" } });
    const on = await mountSettings();
    const light = () => on.get('[data-testid="server-light"]');
    expect(light().attributes("data-state")).toBe("connected");
    expect(light().attributes("aria-label")).toBe("Connected");
    expect(light().classes()).toContain("text-ok");
    expect(on.findAll("section").find((x) => x.find("h3").text().startsWith("Server"))!.find("h3").text()).toContain("●");

    vi.stubGlobal("fetch", async () => {
      throw new TypeError("offline");
    });
    const off = await mountSettings();
    const dead = off.get('[data-testid="server-light"]');
    expect(dead.attributes("data-state")).toBe("offline");
    expect(dead.attributes("aria-label")).toBe("Offline");
    expect(dead.classes()).toContain("text-danger");
  });
});

describe("Settings: EQ and loudness", () => {

  it("toggles EQ through the core", async () => {
    const w = await mountSettings();
    const eq = w.findAll("label").find((l) => l.text() === "EQ enabled")!.get('[role="switch"]');
    expect(eq.attributes("aria-checked")).toBe("true");
    await eq.trigger("click");
    await settle();
    expect(tauri.callsTo("set_eq_enabled")).toEqual([{ enabled: false }]);
    expect(useDspStore().eqEnabled).toBe(false);
  });

  it("draws a node per band in the shared graph editor and can add and remove", async () => {
    const w = await mountSettings();
    expect(w.findAll('[data-testid="eq-node"]')).toHaveLength(1);
    await button(w, /Add band/).trigger("click");
    await settle();
    expect(w.findAll('[data-testid="eq-node"]')).toHaveLength(2);
    // Adding selects the new band, so the panel's Remove acts on it.
    await w.get('[data-testid="eq-band-panel"] button[aria-label="Remove band"]').trigger("click");
    await settle();
    expect(w.findAll('[data-testid="eq-node"]')).toHaveLength(1);
  });

  it("disables Add band at the eight-band limit", async () => {
    const w = await mountSettings();
    for (let i = 0; i < 10; i++) useDspStore().addBand();
    await settle();
    expect(useDspStore().rows.length).toBe(8);
    expect(button(w, /Add band/).attributes("disabled")).toBeDefined();
  });

  /** Selects the first node, so the editor's numeric panel edits that band. */
  async function selectFirstBand(w: Awaited<ReturnType<typeof mountSettings>>) {
    await w.findAll('[data-testid="eq-node"]')[0].trigger("focus");
    await settle();
    return w.get('[data-testid="eq-band-panel"] input[type="number"]').element as HTMLInputElement;
  }

  it("edits the selected band's frequency and applies it live", async () => {
    const w = await mountSettings();
    const freq = await selectFirstBand(w);
    freq.value = "2500";
    freq.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    const last = tauri.callsTo("set_eq_bands").at(-1) as { bands: { freq: number }[] };
    expect(last.bands[0].freq).toBe(2500);
  });

  it("clamps an out-of-range band value to the usable limit", async () => {
    // The graph editor constrains rather than rejecting: it writes back what
    // was actually applied, so the input can never disagree with the engine.
    const w = await mountSettings();
    const freq = await selectFirstBand(w);
    freq.value = "5";
    freq.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(freq.value).toBe(String(EQ_LIMITS.freqMin));
    const last = tauri.callsTo("set_eq_bands").at(-1) as { bands: { freq: number }[] };
    expect(last.bands[0].freq).toBe(EQ_LIMITS.freqMin);
  });

  it("toggles loudness normalisation and validates the target", async () => {
    const w = await mountSettings();
    const loudness = w.findAll("section").find((x) => x.find("h3").text() === "Loudness normalization")!;
    await loudness.get('[role="switch"]').trigger("click");
    await settle();
    expect(tauri.callsTo("set_loudness_enabled")).toEqual([{ enabled: true }]);

    const target = loudness.get('input[type="number"]').element as HTMLInputElement;
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

describe("Settings: crossfeed", () => {
  async function mountWithCrossfeed(crossfeed: Record<string, unknown>, over: Partial<ReturnType<typeof makeState>> = {}) {
    tauri
      .on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14, crossfeed })
      .on("get_output_devices", devices)
      .on("get_output_device", null)
      .on("dop_status", DEFAULT_DOP);
    const { wrapper } = mountApp(SettingsView, {}, {}, () => {
      usePlayerStore().raw = makeState({ status: "playing", output_path: "pcm-shared", ...over });
    });
    await useDspStore().init();
    await settle();
    return wrapper;
  }
  const section = (w: Awaited<ReturnType<typeof mountSettings>>) =>
    w.findAll("section").find((x) => x.find("h3").text() === "Crossfeed")!;
  const lastSent = () => tauri.callsTo("set_crossfeed").at(-1) as { settings: Record<string, unknown> } | undefined;

  it("is off by default, on Bauer, and turns on through the core", async () => {
    const w = await mountSettings();
    const sw = section(w).get('[role="switch"]');
    expect(sw.attributes("aria-checked")).toBe("false");
    expect(select(w, "Crossfeed preset").textContent).toContain("Bauer");
    expect(section(w).get('[data-testid="crossfeed-cutoff"]').text()).toBe("700 Hz");
    expect(section(w).get('[data-testid="crossfeed-feed"]').text()).toBe("4.5 dB");
    await sw.trigger("click");
    await settle();
    expect(lastSent()).toEqual({ settings: { enabled: true, preset: "bauer", cutoff_hz: 700, feed_db: 4.5 } });
  });

  it("offers the three classic presets and Custom (no Linkwitz until its values are verified)", async () => {
    const w = await mountSettings();
    await openSelect(select(w, "Crossfeed preset"));
    expect(optionLabels()).toEqual(["Bauer", "Chu Moy", "Jan Meier", "Custom"]);
  });

  it("choosing a preset fills the cutoff and feed with its values", async () => {
    const w = await mountWithCrossfeed({ enabled: true, preset: "bauer", cutoff_hz: 700, feed_db: 4.5 });
    await openSelect(select(w, "Crossfeed preset"));
    pick(options().find((o) => o.textContent?.trim() === "Jan Meier")!);
    await settle();
    expect(lastSent()).toEqual({ settings: { enabled: true, preset: "meier", cutoff_hz: 650, feed_db: 9.5 } });
    expect(section(w).get('[data-testid="crossfeed-cutoff"]').text()).toBe("650 Hz");
    expect(section(w).get('[data-testid="crossfeed-blurb"]').text()).toContain("Corda");
  });

  it("moving a slider switches to Custom, starting from the preset's values", async () => {
    const w = await mountWithCrossfeed({ enabled: true, preset: "chu_moy", cutoff_hz: 700, feed_db: 6 });
    const feed = section(w).get('[role="slider"][aria-label="Crossfeed feed"]').element as HTMLElement;
    feed.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    await settle();
    expect(lastSent()).toEqual({ settings: { enabled: true, preset: "custom", cutoff_hz: 700, feed_db: 6.5 } });
    expect(select(w, "Crossfeed preset").textContent).toContain("Custom");
  });

  it("says it is bypassed on exclusive output", async () => {
    const w = await mountWithCrossfeed({ enabled: true, preset: "bauer", cutoff_hz: 700, feed_db: 4.5 }, { output_path: "pcm-exclusive" });
    expect(section(w).get('[data-testid="crossfeed-note"]').text()).toContain("Bypassed");
  });

  it("shows in the signal path before the EQ", async () => {
    const { audioPathLabel } = await import("../signalPath");
    const label = audioPathLabel(
      makeTrack(),
      { activeFormat: "flac", isDopExclusive: false, isBitPerfect: false, isExclusive: false },
      { crossfeed: { enabled: true, preset: "meier" }, eqEnabled: true, activeBands: [{}], loudnessEnabled: false, loudnessTarget: -14 },
    );
    expect(label).toContain("PCM shared · Crossfeed Jan Meier · EQ 1 bands");
  });
});

describe("Settings: limiter", () => {
  /**
   * Mounts Settings with the limiter enabled (off is the default, like
   * loudness normalization) and a seeded player snapshot, so the meter has
   * a reading to show.
   */
  async function mountWithState(over: Partial<ReturnType<typeof makeState>> = {}) {
    tauri
      .on("get_dsp_settings", { eq_bands: [], eq_enabled: true, loudness_enabled: false, loudness_target: -14, limiter_enabled: true })
      .on("get_output_devices", devices)
      .on("get_output_device", null)
      .on("dop_status", DEFAULT_DOP);
    const { wrapper } = mountApp(SettingsView, {}, {}, () => {
      usePlayerStore().raw = makeState({ status: "playing", output_path: "pcm-shared", ...over });
    });
    await useDspStore().init();
    await settle();
    return wrapper;
  }
  const section = (w: Awaited<ReturnType<typeof mountSettings>>) =>
    w.findAll("section").find((x) => x.find("h3").text() === "Limiter")!;

  it("is off by default, like loudness normalization, and can be turned on through the core", async () => {
    const w = await mountSettings();
    const sw = section(w).get('[role="switch"]');
    expect(sw.attributes("aria-checked")).toBe("false");
    await sw.trigger("click");
    await settle();
    expect(tauri.callsTo("set_limiter_enabled")).toEqual([{ enabled: true }]);
    expect(useDspStore().limiterEnabled).toBe(true);
  });

  it("shows no reading while off", async () => {
    const w = await mountSettings();
    expect(section(w).find('[data-testid="limiter-gr-idle"]').exists()).toBe(true);
    expect(section(w).find('[data-testid="limiter-gr"]').exists()).toBe(false);
  });

  it("shows no reading when enabled but the engine reports none", async () => {
    const w = await mountWithState({ limiter_gr_db: null });
    expect(section(w).find('[data-testid="limiter-gr-idle"]').exists()).toBe(true);
    expect(section(w).find('[data-testid="limiter-gr"]').exists()).toBe(false);
  });

  it("colours the bar green, yellow then red as the reduction deepens", async () => {
    for (const [db, want] of [
      [0.5, "ok"],
      [4, "warn"],
      [9, "bad"],
    ] as const) {
      const w = await mountWithState({ limiter_gr_db: db });
      expect(section(w).get('[data-testid="limiter-gr"]').attributes("data-state")).toBe(want);
    }
  });

  it("reads the depth out and fills the bar against a 12 dB scale", async () => {
    const w = await mountWithState({ limiter_gr_db: 9 });
    expect(section(w).get('[data-testid="limiter-gr"]').text()).toContain("−9.0 dB");
    expect(section(w).get('[data-testid="limiter-bar"]').attributes("style")).toContain("75%");
  });

  it("says it is bypassed on an exclusive stream", async () => {
    const w = await mountWithState({ output_path: "dop-exclusive", limiter_gr_db: null });
    expect(section(w).get('[data-testid="limiter-note"]').text()).toContain("Bypassed");
  });

  it("says peaks are soft-clipped instead when it is off", async () => {
    const w = await mountWithState({ limiter_gr_db: 0 });
    await section(w).get('[role="switch"]').trigger("click");
    await settle();
    expect(section(w).get('[data-testid="limiter-note"]').text()).toContain("soft-clipped");
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

  const GB = 1024 * 1024 * 1024;
  const sizeOptions = [512 * 1024 * 1024, GB, 2 * GB];

  it("shows the cache size, its limit, free disk space, and clears it", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    let stats = { bytes: 5 * 1024 * 1024, files: 42, max_bytes: 2 * GB, size_options: sizeOptions, free_bytes: 100 * GB };
    tauri.on("artwork_cache_stats", () => stats).on("clear_artwork_cache", () => {
      stats = { ...stats, bytes: 0, files: 0 };
      return 42;
    });
    mockFetch({ "/api/health": { status: "ok" }, "/api/albums": { items: [], page: 1, per_page: 500, total: 0 }, "/api/artists": [] });
    const w = await mountSettings();
    expect(w.text()).toContain("42 images · 5.0 MB of 2.0 GB");
    expect(w.get('[data-testid="artwork-cache-free"]').text()).toBe("100.0 GB free on disk");
    await button(w, "Clear cache").trigger("click");
    await settle();
    expect(tauri.callsTo("clear_artwork_cache")).toHaveLength(1);
    expect(w.text()).toContain("0 images · 1 KB of 2.0 GB");
    expect(button(w, "Clear cache").attributes("disabled")).toBeDefined();
  });

  it("offers 512 MB / 1 GB / 2 GB and changes the limit through the core", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const stats = { bytes: 5 * 1024 * 1024, files: 42, max_bytes: 512 * 1024 * 1024, size_options: sizeOptions, free_bytes: 100 * GB };
    tauri
      .on("artwork_cache_stats", () => stats)
      .on("set_artwork_cache_max_bytes", () => ({ ...stats, max_bytes: 2 * GB }));
    mockFetch({ "/api/health": { status: "ok" }, "/api/albums": { items: [], page: 1, per_page: 500, total: 0 }, "/api/artists": [] });
    const w = await mountSettings();
    await openSelect(select(w, "Album art cache limit"));
    expect(optionLabels()).toEqual(["512.0 MB", "1.0 GB", "2.0 GB"]);
    await pick(options()[2]!);
    await settle();
    expect(tauri.callsTo("set_artwork_cache_max_bytes").at(-1)).toMatchObject({ max_bytes: 2 * GB });
    expect(w.text()).toContain("of 2.0 GB");
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

describe("Settings: logs", () => {
  it("reveals the log folder in Finder", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {}; // the section is app-only
    const w = await mountSettings();
    await w.get('[data-testid="reveal-logs"]').trigger("click");
    await settle();
    expect(tauri.callsTo("reveal_logs")).toHaveLength(1);
  });
});

