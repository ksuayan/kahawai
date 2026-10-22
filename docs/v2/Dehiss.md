# Kahawai De-hiss — Stage 1 (Implemented)

Date: 2026-10-05. Status: implemented, tested, wired into the engine/UI.
Spec: `~/workspace/goals/music-streaming-server-and-player/files/kahawai-dehiss-dsp-spec.md`.

## What it is

A 4-band downward expander for tape and vinyl hiss. It turns the quiet parts down where the signal
drops into the hiss; it does not remove noise from the music itself. Second in the DSP chain, after
de-click.

## Implementation

- `crates/kahawai-player-core/src/dsp/dehiss.rs`: 4-band LR4 crossover, per-band envelope followers
  (stereo-linked), 2:1 ratio with 6 dB knee as v1 constants.
- Settings: threshold (dBFS), per-band trim, amount, attack, release, dry/wet. All off by default.
- Engine: `DspSettings.dehiss`, `set_dehiss` (Player + EngineController + EngineCommand), chain wiring,
  `BLOCKER_DEHISS` for Auto exclusive.
- UI: `DehissSection.vue` in Experimental; `signalPath.ts` shows "De-hiss".

## Non-items (explicit)

- No FFT, no spectral subtraction, no neural approaches (spec defers rustfft).
- No per-stage LUFS meters; dry/wet is the v1 A/B.
- No Learn function (the trim array is the placeholder for it).

## Verification

- Unit tests in `dehiss.rs` (`#[cfg(test)]`): band split, expander curve, stereo linking, bypass.
- `cargo test -p kahawai-player-core`: green. `cargo clippy --all-targets -- -D warnings`: clean.
- Listening acceptance (spec §7) remains for the Mac: A/B on a known hissy tape.
