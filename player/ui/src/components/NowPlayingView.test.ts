import { describe, expect, it } from "vitest";
import { makeAlbum, makeState, makeTrack } from "../test/fixtures";
import { mountApp, settle } from "../test/helpers";
import { tauri } from "../test/tauri-mock";
import { useDspStore } from "../stores/dsp";
import { useLibraryStore } from "../stores/library";
import { useNavStore } from "../stores/nav";
import { usePlayerStore } from "../stores/player";
import NowPlayingView from "./NowPlayingView.vue";

async function boot(state = makeState({ status: "playing" }), prepare?: () => void) {
  tauri.on("get_state", state);
  const { wrapper } = mountApp(NowPlayingView, {}, {}, prepare);
  await usePlayerStore().init();
  await settle();
  return wrapper;
}

describe("NowPlayingView", () => {
  it("says nothing is playing when idle", async () => {
    const w = await boot(makeState({ track: null, queue_ids: [], queue_index: null }));
    expect(w.text()).toContain("Nothing playing.");
    expect(w.find('[role="slider"]').exists()).toBe(false);
  });

  it("shows the title, artist and album", async () => {
    const w = await boot(makeState({ status: "playing", track: makeTrack({ title: "Smoke Signals", artist: "Phoebe Bridgers", album: "Stranger in the Alps" }) }));
    expect(w.get('[data-testid="np-title"]').text()).toBe("Smoke Signals");
    expect(w.text()).toContain("Phoebe Bridgers");
    expect(w.text()).toContain("Stranger in the Alps");
  });

  it("falls back to 'Unknown artist'", async () => {
    const w = await boot(makeState({ track: makeTrack({ artist: null as never }) }));
    expect(w.text()).toContain("Unknown artist");
  });

  it("updates live when the track changes", async () => {
    const w = await boot(makeState({ status: "playing", track: makeTrack({ title: "First" }) }));
    tauri.emit("player-state", makeState({ status: "playing", track: makeTrack({ title: "Second" }) }));
    await settle();
    expect(w.get('[data-testid="np-title"]').text()).toBe("Second");
  });

  it("shows the cover art of the track's album", async () => {
    const album = makeAlbum({ id: 8, artwork_hash: "feed01" });
    const w = await boot(makeState({ status: "playing", track: makeTrack({ album_id: 8 }) }), () => {
      useLibraryStore().albums = [album];
    });
    expect(w.get("img").attributes("src")).toContain("/feed01");
  });

  describe("audio path summary", () => {
    it("describes source → stream → output, with active EQ bands and loudness", async () => {
      const w = await boot(
        makeState({ status: "playing", format: "passthrough", output_path: "pcm-shared", track: makeTrack({ format: "m4a", bit_depth: 24, sample_rate: 96000 }) }),
        () => {
          const dsp = useDspStore();
          dsp.eqEnabled = true;
          dsp.rows = [
            { band_type: "peaking", freq: 1000, gain_db: 3, q: 1, enabled: true },
            { band_type: "peaking", freq: 3000, gain_db: -2, q: 1, enabled: true },
            { band_type: "peaking", freq: 5000, gain_db: 1, q: 1, enabled: false },
          ];
          dsp.loudnessEnabled = true;
          dsp.loudnessTarget = -14;
        },
      );
      expect(w.text()).toContain("M4A · 24/96k → PASSTHROUGH → PCM shared · EQ 2 bands · Loudness -14 LUFS");
    });

    it("omits DSP for exclusive DoP (it is bit-perfect) and shows the DoP badge", async () => {
      const w = await boot(
        makeState({ status: "playing", format: "dop", output_path: "dop-exclusive", track: makeTrack({ format: "dsf", bit_depth: 1, sample_rate: 2822400 }) }),
        () => {
          useDspStore().eqEnabled = true;
          useDspStore().rows = [{ band_type: "peaking", freq: 1000, gain_db: 3, q: 1, enabled: true }];
        },
      );
      expect(w.text()).toContain("→ DOP → Exclusive DoP · bit-perfect");
      expect(w.text()).not.toContain("EQ ");
      expect(w.text()).toContain("Exclusive DoP");
    });

    it("shows AUTO when no format has been chosen", async () => {
      const w = await boot(makeState({ status: "playing", format: null }));
      expect(w.text()).toContain("→ AUTO →");
    });
  });

  it("flags an unplayable track with the reason", async () => {
    const w = await boot(makeState({ status: "stopped", track: makeTrack({ missing: true }) }));
    expect(w.text()).toContain("File missing from disk");
  });

  it("shows the engine error", async () => {
    const w = await boot(makeState({ status: "stopped", error: "decode failed" }));
    expect(w.get('[role="alert"]').text()).toBe("decode failed");
  });

  it("embeds the shared seek, volume and transport controls", async () => {
    const w = await boot(makeState({ status: "paused", position_ms: 83_000, volume: 0.4, track: makeTrack({ duration_ms: 318_742 }) }));
    expect(w.get('[data-testid="elapsed"]').text()).toBe("1:23");
    expect(w.get('[role="slider"][aria-label="Volume"]').attributes("aria-valuenow")).toBe("40");
    expect(w.get('[data-testid="play-pause"]').attributes("aria-label")).toBe("Play");
    // No stop button on the full page (that lives in the bar).
    expect(w.findAll("button").some((b) => b.attributes("aria-label") === "Stop")).toBe(false);
    await w.get('[role="slider"][aria-label="Seek"]').trigger("keydown", { key: "ArrowRight" });
    await settle();
    expect(tauri.callsTo("seek_ms")).toEqual([{ ms: 84_000 }]);
  });

  it("navigates back to the library and to the EQ settings", async () => {
    const w = await boot();
    await w.findAll("button").find((b) => b.text().includes("Library"))!.trigger("click");
    expect(useNavStore().view.name).toBe("albums");
    await w.findAll("button").find((b) => b.text().includes("EQ"))!.trigger("click");
    expect(useNavStore().view.name).toBe("settings");
  });
});
