//! Analog character: an optional tube or transistor "warmth" stage for the
//! shared PCM path. (Plan: docs/v1/Analog-Emulation.md, Phases 1 and 2.)
//!
//! Signal flow per channel, all in place on interleaved f32:
//!
//! ```text
//! x -> drive -> [oversample -> curve (with ADAA) -> decimate]
//!        -> DC blocker -> output trim & gain match ─┐
//! x -> latency-matched delay ─────────────────────── mix -> out
//! ```
//!
//! The warm-triode curve is computed from Koren's triode equations (a 12AX7
//! stage with a resistive load); the solid-state curve is a symmetric tanh.
//! It models *character* (level-dependent harmonics and a soft knee), not a
//! specific circuit. Pure Rust, no platform imports. PCM only: the DoP and
//! bit-perfect paths never call it.
//!
//! - [`settings`]: the flavours and [`AnalogSettings`].
//! - [`models`]: the device models and their transfer tables.
//! - [`shaper`]: the curves, drive, sag and transformer colour.
//! - [`oversample`]: anti-aliasing and oversampling.
//! - [`stage`]: [`AnalogStage`], which ties them together.

mod models;
mod oversample;
mod settings;
mod shaper;
mod stage;
#[cfg(test)]
mod tests;

pub use models::{koren_plate_current, triode_plate_voltage, triode_table, TubeTable};
pub use oversample::{anti_alias_plan, oversample_factor, AntiAlias};
pub use settings::{AnalogFlavour, AnalogSettings, AntiAliasChoice};
pub use stage::{AnalogStage, AnalogStatus};
