//! The PCM DSP chain, one stage per file. (Spec: kahawai-player-design.md §5.)
//!
//! - [`crossfeed`]: headphone crossfeed (bs2b).
//! - [`eq`]: the parametric EQ, built on the [`biquad`] filters.
//! - [`analog`]: analog character (tape, tube and the rest).
//! - [`loudness`]: loudness metering, normalization and the track pre-scan.
//! - [`gain_ramp`]: click-free gain changes on track boundaries.
//! - [`limiter`]: the look-ahead limiter and headroom guard at the end of the chain.
//!
//! Every stage implements [`DspStage`], and every public item is re-exported
//! here, so `crate::dsp::ParametricEq` and the rest keep their paths.
//!
//! Pure Rust, no platform imports. **PCM only**: the DoP path bypasses this
//! module entirely — the engine never calls into it for DoP streams (see
//! `engine.rs`; `engine_tests` asserts bit-identical DoP passthrough with
//! EQ enabled and volume at 50%).

pub mod analog;
mod biquad;
pub mod crossfeed;
mod eq;
mod gain_ramp;
mod limiter;
mod loudness;
#[cfg(test)]
mod test_util;

pub use eq::{
    max_boost_db, usable_freq, validate_bands, EqBand, EqBandType, ParametricEq,
    EQ_PREAMP_RANGE_DB, MAX_EQ_BANDS, NYQUIST_FRACTION,
};
pub use gain_ramp::GainRamp;
pub use limiter::{
    headroom_guard, LookaheadLimiter, GUARD_THRESHOLD, LIMITER_CEILING, LIMITER_LOOKAHEAD_MS,
    LIMITER_RELEASE_MS,
};
pub use loudness::{
    integrated_lufs, plan_gain_db, scan_track_levels, scan_track_lufs, LoudnessMeter, LoudnessNorm,
    DEFAULT_LOUDNESS_TARGET, HEADROOM_MARGIN_DB, MAX_LOUDNESS_GAIN_DB, MIN_LOUDNESS_GAIN_DB,
};

/// One stage of the PCM chain (EQ, analog character, later others). All
/// stages work in place on interleaved f32 and are bit-transparent when off.
pub trait DspStage: Send {
    /// The sample rate the audio has when it reaches this stage.
    fn prepare(&mut self, sample_rate: u32);
    /// Process interleaved samples in place.
    fn process(&mut self, interleaved: &mut [f32], channels: usize);
    /// Delay the stage adds to the signal, in frames.
    fn latency_frames(&self) -> u32 {
        0
    }
    /// Forget history (a new track, a seek).
    fn reset(&mut self);
}
