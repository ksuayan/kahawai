# Kahawai Crossfeed DSP — Design Specification

**Status:** implemented on the `dsp-crossfeed` branch (`crates/kahawai-player-core/src/crossfeed.rs`, `player/ui/src/components/CrossfeedSection.vue`), with these deviations from the text below:

- **No delay line; `latency_frames()` is 0.** The bs2b reference (libbs2b 3.1.0 `init()` / `cross_feed_d()`) has no delay: the timing cue comes from the filters' phase. §2.1's `LP_tau` and §4's delay-line latency describe a different topology; the code follows the reference.
- **No Linkwitz preset yet.** Its 1971 circuit values could not be verified, and §2.2 forbids a preset under that name that isn't digitized from the real circuit. Tracked in `docs/Backlog.md`.
- **Custom feed range is 0.5–15 dB** (not 0–12), close to bs2b's own limits and with headroom above Jan Meier's 9.5 dB.
- **Bit-perfect blocker.** Like EQ, loudness, analog warmth and the limiter, an enabled crossfeed keeps Auto off exclusive output (`BLOCKER_CROSSFEED`).
- **Sliders always visible,** showing the preset's values; moving one switches the menu to Custom (§2.3's convention).

Originally written against `main` at `2a6997c` (2026-09-29) as a spec only.

## 1. Goal

Give headphone listeners relief from "super stereo" — the hard left/right split that makes some recordings (especially early stereo) fatiguing over long sessions. The stage blends a low-passed, slightly delayed copy of each channel into the other, approximating what each ear would hear from real speakers.

Non-goals: HRTF/speaker virtualization (a separate, larger project), room simulation, multichannel downmixing.

## 2. Algorithm

### 2.1 BS2B topology — Bauer, Chu Moy, and Meier presets

Per channel, in place on interleaved f32:

```text
out_L = in_L + feed * LP_tau(in_R)
out_R = in_R + feed * LP_tau(in_L)
```

- `LP`: first-order low-pass at the preset's cutoff frequency.
- `tau`: a fixed interaural delay of a few hundred microseconds (head-width scale). Not user-exposed in v1; reported via `latency_frames()`.
- `feed`: the preset's feed level, converted from dB to linear.

This is the topology every BS2B-based implementation (foobar2000, EasyEffects, Roon, Neutron) uses; the three presets differ only in two numbers.

### 2.2 Linkwitz preset

Siegfried Linkwitz's December 1971 *Audio* article ("Improved Headphone Listening") publishes an RC crossfeed network whose response curves (Fig. 2) are the reference. Chu Moy's well-known variant is itself a tweak of this circuit, so the Linkwitz preset is the ancestor of two of the other three.

Implementation: derive the analog prototype's transfer function from the published schematic and digitize it with the bilinear transform inside `prepare()`. If the schematic's component values prove ambiguous, fall back to fitting the BS2B topology (cutoff + feed + delay) to the article's published response curves, and say so in the code comment. Either way the preset must be labeled in the UI as "digitized from the published 1971 circuit" — never imply it *is* the analog circuit.

### 2.3 Preset table

| Preset | Cutoff | Feed | Character |
|---|---|---|---|
| Bauer (default) | 700 Hz | 4.5 dB | Closest to virtual speakers at 30°, 3 m. The subtle one. |
| Chu Moy | 700 Hz | 6.0 dB | Moy's tweak of the Linkwitz circuit. A touch more present. |
| Jan Meier | 650 Hz | 9.5 dB | From Meier's Corda amps. The strongest of the three classics. |
| Linkwitz | (circuit) | (circuit) | The 1971 original, digitized. The ancestor. |
| Custom | 200–2000 Hz slider | 0–12 dB slider | Manual tuning. |

Cutoff/feed values for the first three are BS2B's published presets, confirmed against the EasyEffects docs, the bs2b-crossfeed-macos project, and bs2b's own release notes. Selecting a preset fills the cutoff/feed fields with its values (visible to the user); editing either field flips the pulldown to Custom — the same convention BS2B-based UIs use.

## 3. Settings model

New file `crates/kahawai-player-core/src/crossfeed.rs`, mirroring the `AnalogSettings` pattern (`analog.rs:132`):

```rust
/// Which classic circuit this stage imitates.
pub enum CrossfeedPreset { Bauer, ChuMoy, Meier, Linkwitz, Custom }

/// User-facing settings; persisted in `engine-settings.json`.
#[serde(default)]
pub struct CrossfeedSettings {
    pub enabled: bool,          // default false
    pub preset: CrossfeedPreset, // default Bauer
    /// Low-pass cutoff of the crossfed path, Hz. From preset; editable in Custom.
    pub cutoff_hz: f32,         // default 700.0
    /// Feed level of the crossfed path, dB. From preset; editable in Custom.
    pub feed_db: f32,           // default 4.5
}
```

- `impl Default`, plus `clamped()` rejecting non-finite values and pulling ranges into bounds (same contract as `AnalogSettings::clamped`).
- `DspSettings` (`engine.rs`) gains `#[serde(default)] pub crossfeed: CrossfeedSettings`. Off by default, so Auto bit-perfect and native DSD behavior on a fresh install are unchanged.

## 4. `DspStage` implementation

`pub struct CrossfeedStage` implements `DspStage` (`dsp.rs:218`):

- `prepare(sample_rate)` — (re)design the low-pass biquad (or the Linkwitz bilinear-transform biquads) at the new rate. Crossfeed's psychoacoustic constants are defined by head geometry, not sample rate, so the *design* scales while the *perception* stays put. This is what makes the stage cheap and correct on high-resolution audio with no special-casing.
- `process(&mut [f32], channels)` — stereo only. Any other channel count passes through untouched (never invent behavior for layouts the presets weren't designed for).
- `latency_frames()` — the delay-line length in frames (sub-millisecond); 0 when bypassed. Feeds the same latency accounting the look-ahead limiter already uses.
- `reset()` — clear delay lines and filter state (track change, seek).
- Bit-transparency: when `enabled` is false, return before touching the buffer — the same early-return bypass `AnalogStage::process` uses.
- Enable/disable ramps over ~15 ms (`EQ_RAMP_SECONDS` in `dsp.rs`) so toggling never clicks.

Cost: ~10 multiply-adds per sample per channel plus a delay line of dozens of samples. At 192 kHz stereo this is single-digit millions of MAC/s — the lightest stage in the chain by a wide margin, far below the oversampled analog stage.

## 5. Engine integration

- New field `crossfeed: CrossfeedStage` on the engine struct, with `set_crossfeed()` mirroring `set_analog()` (`engine.rs:1131`): no-op when settings are unchanged, live-applied otherwise.
- Chain position: **first, before EQ**. Rationale: crossfeed simulates the speaker acoustics; EQ corrects the headphone's own response, so it should correct the signal as the ear will actually receive it. This amends the fixed-order comment at `engine.rs:1670`.
- Never invoked on the DoP / bit-perfect path — same exclusion the analog stage documents (`analog.rs` module docs: "the DoP and bit-perfect paths never call it").
- `signalPath.ts` gains the crossfeed entry so the signal-path display stays truthful.

## 6. Tauri API and frontend

- New `set_crossfeed(settings)` command mirroring `set_analog` (`player/ui/src/tauri.ts:242`); `DspSettings` in `types.ts` extended.
- `types.ts`: `CROSSFEED_PRESET_INFO: Record<CrossfeedPreset, { label, blurb }>` mirroring `FLAVOUR_INFO` (`types.ts:212`). Suggested blurbs (plain, honest, no superlatives):
  - Bauer — "The 1961 original via BS2B. Virtual speakers at 30 degrees."
  - Chu Moy — "A DIY-era tweak of Linkwitz's circuit. A touch more present."
  - Jan Meier — "From Jan Meier's Corda headphone amps. The strongest classic."
  - Linkwitz — "Linkwitz's 1971 circuit, digitized. The ancestor of the others."
- New `CrossfeedSection.vue` in the Sound settings, following `AnalogSection.vue`: a `UiSelect` pulldown with the four presets plus Custom, the blurb line under it, cutoff/feed sliders visible in Custom mode, and a single enable toggle. No A/B slot machinery in v1 — a bypass toggle is enough to hear what the stage does (the analog store's A/B pattern is reusable later if listeners ask for it).

## 7. Tests (`kahawai-player-core`)

- Bypass is bit-transparent: `enabled: false` → output bytes identical to input.
- Preset table: each named preset resolves to its published cutoff/feed.
- Reference check: output matches a direct scalar implementation of §2.1 on a synthetic stereo signal (not just "it changed something").
- Non-stereo passthrough: 1- and 6-channel buffers untouched.
- Sample-rate independence: the −3 dB point of the crossfed path measures the same at 44.1 kHz and 192 kHz.
- `latency_frames()` equals the delay-line length; 0 when bypassed.
- `clamped()` rejects NaN/infinite; no NaN/inf in output on silence or denormals.
- Linkwitz preset: frequency response matches the 1971 article's curves within a stated tolerance (tolerance documented in the test, not hand-waved).

## 8. Docs

- New plan doc `docs/v1/Crossfeed.md` (sibling to `Analog-Emulation.md`) recording the preset sources and the Linkwitz digitization method.
- In-app strings follow the Editorial Style Guide: no claim the UI can't verify ("digitized from," not "identical to").

## 9. Phases

- **Phase 1 (this spec):** 4 presets + custom, stereo only, fixed first-in-chain position, bypass toggle.
- **Phase 2 (optional, not spec'd):** SPL Phonitor-style angle/center parameterization; A/B slot comparison; Linkwitz response refinement against measured hardware.
