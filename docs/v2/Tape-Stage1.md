# Kahawai Tape — Stage 1 (Implemented)

Date: 2026-10-05. Status: implemented, tested, wired into the engine/UI.
Spec: `~/workspace/goals/music-streaming-server-and-player/files/kahawai-tape-dsp-spec.md`.
TODO: `~/workspace/goals/music-streaming-server-and-player/files/kahawai-tape-dsp-todo.md` (TAPE-1..TAPE-11).

## What it is

Tape color, first stage: record/repro EQ (NAB/IEC), gentle memoryless saturation with ADAA/oversampling,
head bump, wow/flutter/drift, shaped hiss + asperity. The broad strokes of what a tape path does to the
sound; it does not model any specific machine. Runs after analog, last among the color stages.

## Implementation

- `crates/kahawai-player-core/src/dsp/tape.rs`: exact first-order NAB/IEC sections (bilinear),
  asymmetric tanh with ADAA (Eco) / 2x/4x Kaiser-FIR oversampling (Standard/High), head bump + notch,
  wow/flutter/drift on fractional delay (L/R decorrelated), deterministic xorshift noise.
- Settings: drive, trim, saturation, bias, speed, EQ standard, head bump, wow/flutter rate+depth,
  drift, hiss, asperity, quality, dry/wet. All off by default; hiss exactly zero at minimum.
- Engine: `DspSettings.tape`, `set_tape` (Player + EngineController + EngineCommand), chain wiring,
  `BLOCKER_TAPE` for Auto exclusive.
- UI: `TapeSection.vue` in Experimental; `signalPath.ts` shows "Tape (Stage 1)".

## Spec deviations (documented)

- Latency: spec §2.4 says sub-millisecond, but worst-case wow (0.5% @ 0.2 Hz) needs ~5 ms at 44.1 kHz.
  `latency_frames()` reports the honest worst case.

## Non-items (explicit)

- Stage 2: Jiles–Atherton hysteresis core.
- Stage 3: per-machine voicings.
- No hardware-modeling claims in the UI.

## Verification

- Unit tests in `tape.rs` (`#[cfg(test)]`): EQ inverse flatness, saturation curve, hiss off at minimum,
  determinism, bypass.
- `cargo test -p kahawai-player-core`: green. `cargo clippy --all-targets -- -D warnings`: clean.
- Listening acceptance (spec §7) remains for the Mac: A/B procedure in the README.
