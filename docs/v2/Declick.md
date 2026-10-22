# Kahawai De-click — Stage 1 (Implemented)

Date: 2026-10-05. Status: implemented, tested, wired into the engine/UI.
Spec: `~/workspace/goals/music-streaming-server-and-player/files/kahawai-declick-dsp-spec.md`.
TODO: `~/workspace/goals/music-streaming-server-and-player/files/kahawai-declick-dsp-todo.md`.

## What it is

Detect-plus-interpolate click repair for vinyl transfers. First-difference detector against an adaptive
RMS threshold, cubic Hermite (or linear extrapolation for short gaps) repair, internal lookahead FIFO.
First in the DSP chain, on the raw transfer.

## Implementation

- `crates/kahawai-player-core/src/dsp/declick.rs`: per-channel detector, causal RMS (past-only) so a
  transient stands out against preceding quiet, wide-defect skip, 5 ms lookahead (honest latency via
  `latency_frames()`).
- Settings: sensitivity (0..1, log-mapped 6×..1.5× threshold), max width ms, dry/wet. All off by default.
- Engine: `DspSettings.declick`, `set_declick` (Player + EngineController + EngineCommand), chain wiring,
  `BLOCKER_DECLICK` for Auto exclusive.
- UI: `DeclickSection.vue` in Experimental; `signalPath.ts` shows "De-click".

## Spec deviations (documented)

- Sensitivity mapping: spec §2.1 says 40×/6×, but its own §7 test can't fire at those values.
  Implemented log-mapped 6×→1.5× (default 3×).
- Smooth low-diff burst tails are unrepairable by any local method; the spec-literal contract (impulse
  gone, THD unchanged) is what the tests assert.

## Non-items (explicit)

- No crackle/thump handling (different defects, different detectors).
- No neural approaches.

## Verification

- Unit tests in `declick.rs` (`#[cfg(test)]`): synthetic click, chunk boundary, wide defect, drum
  transient, sensitivity extremes, latency, bypass.
- `cargo test -p kahawai-player-core`: green. `cargo clippy --all-targets -- -D warnings`: clean.
- Listening acceptance (spec §7) remains for the Mac: A/B on a known clicky record.
