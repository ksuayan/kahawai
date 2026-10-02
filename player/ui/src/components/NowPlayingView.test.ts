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

    it("bit-perfect: names the path, omits DSP, and shows the badge", async () => {
      const w = await boot(
        makeState({ status: "playing", format: "passthrough", output_path: "pcm-exclusive", track: makeTrack({ format: "flac", bit_depth: 24, sample_rate: 48000, mqa: true, original_sample_rate: 48000 }) }),
        () => {
          useDspStore().eqEnabled = true;
          useDspStore().rows = [{ band_type: "peaking", freq: 1000, gain_db: 3, q: 1, enabled: true }];
          useDspStore().loudnessEnabled = true;
        },
      );
      expect(w.text()).toContain("→ PASSTHROUGH → Bit-perfect · exclusive");
      expect(w.text()).not.toContain("EQ 1 bands");
      expect(w.text()).not.toContain("Loudness");
      expect(w.get('[data-testid="bit-perfect-badge"]').text()).toBe("Bit-perfect");
      expect(w.get('[data-testid="mqa-badge"]').text()).toBe("MQA · 48k");
    });

    it("shows AUTO when no format has been chosen", async () => {
      const w = await boot(makeState({ status: "playing", format: null }));
      expect(w.text()).toContain("→ AUTO →");
    });
  });

  it("labels an MQA track, and not an ordinary one", async () => {
    const w = await boot(makeState({ status: "playing", track: makeTrack({ format: "flac", mqa: true, original_sample_rate: 96000 }) }));
    expect(w.get('[data-testid="mqa-badge"]').text()).toBe("MQA · 96k");
    const plain = await boot(makeState({ status: "playing", track: makeTrack({ mqa: false }) }));
    expect(plain.find('[data-testid="mqa-badge"]').exists()).toBe(false);
  });

  it("flags an unplayable track with the reason", async () => {
    const w = await boot(makeState({ status: "stopped", track: makeTrack({ missing: true }) }));
    expect(w.text()).toContain("File missing from disk");
  });

  it("shows the engine error", async () => {
    const w = await boot(makeState({ status: "stopped", error: "decode failed" }));
    expect(w.get('[role="alert"]').text()).toBe("decode failed");
  });

  it("has no transport, seek, volume or stream-format controls (they live in the bar)", async () => {
    const w = await boot(makeState({ status: "paused", position_ms: 83_000, track: makeTrack({ duration_ms: 318_742 }) }));
    expect(w.find('[data-testid="play-pause"]').exists()).toBe(false);
    expect(w.find('[role="slider"]').exists()).toBe(false);
    expect(w.find('[aria-label="Stream format for this track"]').exists()).toBe(false);
    expect(w.text()).not.toContain("Stream this track as");
  });

  it("offers Add to queue and Add to playlist… buttons instead of a ⋯ menu, and no duplicate source-format badge", async () => {
    const track = makeTrack({ format: "flac", bit_depth: 24, sample_rate: 96000 });
    const w = await boot(makeState({ status: "playing", track }));
    expect(w.get('[data-testid="add-to-queue"]').text()).toContain("Add to queue");
    expect(w.get('[data-testid="add-to-playlist"]').text()).toContain("Add to playlist");
    expect(w.find('[aria-label^="Actions for"]').exists()).toBe(false);
    expect(w.find('[data-testid="play-next"]').exists()).toBe(false); // it is the track playing
    expect(w.get('[data-testid="add-to-queue"] svg').classes().join(" ")).toContain("lucide-list-plus");
    // The source format appears once (inside the path badge), not as a separate tag.
    expect(w.text().match(/FLAC · 24\/96k/g)).toHaveLength(1);
  });

  it("navigates back to the library; EQ is a transport control now, not a separate button", async () => {
    const w = await boot();
    expect(w.findAll("button").some((b) => b.text().trim() === "EQ")).toBe(false);
    await w.findAll("button").find((b) => b.text().includes("Library"))!.trigger("click");
    expect(useNavStore().view.name).toBe("albums");
  });
});
