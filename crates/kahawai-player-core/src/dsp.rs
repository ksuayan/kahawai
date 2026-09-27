//! v1 DSP: parametric EQ + loudness normalization. (Spec:
//! kahawai-player-design.md §5.)
//!
//! Pure Rust, no platform imports. **PCM only**: the DoP path bypasses this
//! module entirely — the engine never calls into it for DoP streams (see
//! `engine.rs`; `engine_tests` asserts bit-identical DoP passthrough with
//! EQ enabled and volume at 50%).

use std::collections::HashMap;

use kahawai_core::{api::StreamFormat, MusicError};
use serde::{Deserialize, Serialize};

use crate::decode::StreamDecoder;
use crate::transport::{StreamOptions, Transport};

// ---------------------------------------------------------------------------
// Biquad core (RBJ "Audio EQ Cookbook")
// ---------------------------------------------------------------------------

/// Normalized biquad coefficients (a0 = 1). f64: at 176/192 kHz a low-
/// frequency band needs coefficients within ~1e-6 of 1.0, which f32 cannot
/// represent well enough (noisy or slightly wrong response).
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    /// |H(e^jw)| at angular frequency `w` (a0 is 1).
    fn magnitude(&self, w: f64) -> f64 {
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }
}

/// The largest boost (dB, never below 0) the band set applies at any
/// frequency: the worst case for headroom. Sampled on a log grid.
pub fn max_boost_db(bands: &[EqBand], sample_rate: u32) -> f32 {
    if bands.is_empty() || sample_rate == 0 {
        return 0.0;
    }
    let designed: Vec<Biquad> = bands.iter().map(|b| design_band(b, sample_rate)).collect();
    let top = (sample_rate as f64 * NYQUIST_FRACTION as f64).max(21.0);
    let mut worst = 1.0f64;
    for i in 0..512 {
        let f = 20.0 * (top / 20.0).powf(i as f64 / 511.0);
        let w = 2.0 * std::f64::consts::PI * f / sample_rate as f64;
        let mag: f64 = designed.iter().map(|b| b.magnitude(w)).product();
        worst = worst.max(mag);
    }
    (20.0 * worst.log10()) as f32
}

/// Direct Form II transposed state, one per channel.
#[derive(Debug, Clone, Copy, Default)]
struct BiquadState {
    z1: f64,
    z2: f64,
}

/// Audio stays f32; only the filter arithmetic is f64.
#[inline]
fn biquad_step(c: &Biquad, s: &mut BiquadState, x: f32) -> f32 {
    let x = x as f64;
    let y = c.b0 * x + s.z1;
    s.z1 = c.b1 * x - c.a1 * y + s.z2;
    s.z2 = c.b2 * x - c.a2 * y;
    y as f32
}

// ---------------------------------------------------------------------------
// Parametric EQ
// ---------------------------------------------------------------------------

/// EQ band type. Wire shape is snake_case for the Tauri bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBandType {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

/// One EQ band. `q` is the RBJ Q for peaking/low-pass/high-pass; for
/// shelves it is the shelf slope S (clamped to 0.1..=3.0).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub band_type: EqBandType,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Maximum simultaneous bands (v1 design decision).
pub const MAX_EQ_BANDS: usize = 8;

fn validate_band(b: &EqBand) -> Result<(), MusicError> {
    if !(10.0..=24_000.0).contains(&b.freq) || !b.freq.is_finite() {
        return Err(MusicError::BadRequest(format!(
            "EQ band frequency out of range: {}",
            b.freq
        )));
    }
    if !(0.1..=18.0).contains(&b.q) || !b.q.is_finite() {
        return Err(MusicError::BadRequest(format!(
            "EQ band Q out of range: {}",
            b.q
        )));
    }
    if !(-24.0..=24.0).contains(&b.gain_db) || !b.gain_db.is_finite() {
        return Err(MusicError::BadRequest(format!(
            "EQ band gain out of range: {}",
            b.gain_db
        )));
    }
    Ok(())
}

/// Validate a band list without touching the live chain (used by the Tauri
/// command so the UI gets an error before anything is sent to the engine).
pub fn validate_bands(bands: &[EqBand]) -> Result<(), MusicError> {
    if bands.len() > MAX_EQ_BANDS {
        return Err(MusicError::BadRequest(format!(
            "at most {MAX_EQ_BANDS} EQ bands"
        )));
    }
    for b in bands {
        validate_band(b)?;
    }
    Ok(())
}

/// Highest band frequency that stays meaningful at `sample_rate`: a fraction
/// of Nyquist, since the RBJ formulas degenerate as w0 approaches pi.
pub const NYQUIST_FRACTION: f32 = 0.45;

/// The frequency a band is actually designed at (its own, capped for the rate).
pub fn usable_freq(freq: f32, sample_rate: u32) -> f32 {
    freq.min(sample_rate as f32 * NYQUIST_FRACTION)
}

/// Design one band at `sample_rate` using the RBJ cookbook.
fn design_band(band: &EqBand, sample_rate: u32) -> Biquad {
    let a = 10f64.powf(band.gain_db as f64 / 40.0);
    let freq = usable_freq(band.freq, sample_rate) as f64;
    let q = band.q as f64;
    let w0 = 2.0 * std::f64::consts::PI * freq / sample_rate as f64;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);

    let (b0, b1, b2, a0, a1, a2) = match band.band_type {
        EqBandType::Peaking => (
            1.0 + alpha * a,
            -2.0 * cw,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cw,
            1.0 - alpha / a,
        ),
        EqBandType::LowShelf => {
            let s = q.clamp(0.1, 3.0);
            let alpha_s = sw / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
            let sq = 2.0 * a.sqrt() * alpha_s;
            (
                a * ((a + 1.0) - (a - 1.0) * cw + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                a * ((a + 1.0) - (a - 1.0) * cw - sq),
                (a + 1.0) + (a - 1.0) * cw + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                (a + 1.0) + (a - 1.0) * cw - sq,
            )
        }
        EqBandType::HighShelf => {
            let s = q.clamp(0.1, 3.0);
            let alpha_s = sw / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
            let sq = 2.0 * a.sqrt() * alpha_s;
            (
                a * ((a + 1.0) + (a - 1.0) * cw + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                a * ((a + 1.0) + (a - 1.0) * cw - sq),
                (a + 1.0) - (a - 1.0) * cw + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * cw),
                (a + 1.0) - (a - 1.0) * cw - sq,
            )
        }
        EqBandType::LowPass => {
            let c = (1.0 - cw) / 2.0;
            (c, 1.0 - cw, c, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
        }
        EqBandType::HighPass => {
            let c = (1.0 + cw) / 2.0;
            (c, -(1.0 + cw), c, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
        }
    };
    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

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

impl DspStage for ParametricEq {
    fn prepare(&mut self, sample_rate: u32) {
        self.set_sample_rate(sample_rate);
    }
    fn process(&mut self, interleaved: &mut [f32], channels: usize) {
        ParametricEq::process(self, interleaved, channels);
    }
    fn reset(&mut self) {
        let rate = self.sample_rate;
        let bands = self.bands.clone();
        self.slots.clear();
        self.primed = false;
        self.mix = if self.enabled { 1.0 } else { 0.0 };
        let designed = self.design_all(&bands);
        self.slots = designed.into_iter().map(Slot::snapped).collect();
        self.sample_rate = rate;
    }
}

/// How long a live change (bands, on/off) takes to fade in, in seconds.
/// Long enough to be free of clicks, short enough to feel immediate.
const EQ_RAMP_SECONDS: f32 = 0.015;

const IDENTITY: Biquad = Biquad {
    b0: 1.0,
    b1: 0.0,
    b2: 0.0,
    a1: 0.0,
    a2: 0.0,
};

/// One filter stage. On a live edit the coefficients glide from `cur` to
/// `target` while the filter state is kept, which is what avoids clicks.
struct Slot {
    cur: Biquad,
    target: Biquad,
    step: Biquad,
    remaining: usize,
    /// Fading toward a pass-through; dropped once it gets there.
    removing: bool,
    /// One state pair per channel.
    states: Vec<BiquadState>,
}

impl Slot {
    fn snapped(c: Biquad) -> Self {
        Self {
            cur: c,
            target: c,
            step: Biquad { b0: 0.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 },
            remaining: 0,
            removing: false,
            states: Vec::new(),
        }
    }

    fn retarget(&mut self, to: Biquad, frames: usize, removing: bool) {
        let n = frames.max(1) as f64;
        self.step = Biquad {
            b0: (to.b0 - self.cur.b0) / n,
            b1: (to.b1 - self.cur.b1) / n,
            b2: (to.b2 - self.cur.b2) / n,
            a1: (to.a1 - self.cur.a1) / n,
            a2: (to.a2 - self.cur.a2) / n,
        };
        self.target = to;
        self.remaining = frames.max(1);
        self.removing = removing;
    }

    #[inline]
    fn advance(&mut self) {
        if self.remaining == 0 {
            return;
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.cur = self.target;
        } else {
            self.cur.b0 += self.step.b0;
            self.cur.b1 += self.step.b1;
            self.cur.b2 += self.step.b2;
            self.cur.a1 += self.step.a1;
            self.cur.a2 += self.step.a2;
        }
    }
}

/// Up-to-8-band parametric EQ over interleaved f32.
///
/// Bypass is bit-transparent: once faded out (or with no bands) `process`
/// does not touch the buffer at all. Edits made while audio is flowing (new
/// bands, on/off) fade in over ~15 ms with the filter state kept, so
/// dragging a control point does not click. Before any audio has passed
/// (a new track or rate), changes apply at once.
pub struct ParametricEq {
    bands: Vec<EqBand>,
    enabled: bool,
    sample_rate: u32,
    slots: Vec<Slot>,
    /// Wet/dry mix: 1 = fully equalized, 0 = untouched. Moves toward the
    /// enabled/disabled target so toggling does not click.
    mix: f32,
    /// Audio has passed through since the last reset; only then is it worth
    /// fading a change.
    primed: bool,
}

impl ParametricEq {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            bands: Vec::new(),
            enabled: true,
            sample_rate,
            slots: Vec::new(),
            mix: 1.0,
            primed: false,
        }
    }

    fn ramp_frames(&self) -> usize {
        ((self.sample_rate as f32 * EQ_RAMP_SECONDS) as usize).max(1)
    }

    fn design_all(&self, bands: &[EqBand]) -> Vec<Biquad> {
        bands.iter().map(|b| design_band(b, self.sample_rate)).collect()
    }

    pub fn set_bands(&mut self, bands: Vec<EqBand>) -> Result<(), MusicError> {
        validate_bands(&bands)?;
        let designed = self.design_all(&bands);
        self.bands = bands;
        if !self.primed {
            self.slots = designed.into_iter().map(Slot::snapped).collect();
            return Ok(());
        }
        let ramp = self.ramp_frames();
        for (i, c) in designed.iter().enumerate() {
            match self.slots.get_mut(i) {
                Some(slot) => slot.retarget(*c, ramp, false),
                None => {
                    // A new band fades in from a pass-through.
                    let mut slot = Slot::snapped(IDENTITY);
                    slot.retarget(*c, ramp, false);
                    self.slots.push(slot);
                }
            }
        }
        for slot in self.slots.iter_mut().skip(designed.len()) {
            slot.retarget(IDENTITY, ramp, true);
        }
        Ok(())
    }

    /// Worst-case boost of the live bands (0 when the EQ is off or flat).
    pub fn max_boost_db(&self) -> f32 {
        if self.enabled {
            max_boost_db(&self.bands, self.sample_rate)
        } else {
            0.0
        }
    }

    pub fn bands(&self) -> &[EqBand] {
        &self.bands
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !self.primed {
            self.mix = if enabled { 1.0 } else { 0.0 };
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Redesign filters when the stream rate changes; state is cleared
    /// (a rate change is a track boundary, never mid-track).
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            let designed = self.design_all(&self.bands);
            self.slots = designed.into_iter().map(Slot::snapped).collect();
            self.primed = false;
            self.mix = if self.enabled { 1.0 } else { 0.0 };
        }
    }

    fn ensure_states(&mut self, channels: usize) {
        for slot in &mut self.slots {
            if slot.states.len() != channels {
                slot.states = vec![BiquadState::default(); channels];
            }
        }
    }

    /// Process interleaved samples in place. Bit-transparent no-op unless
    /// enabled with at least one band (or still fading out).
    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let target_mix = if self.enabled { 1.0 } else { 0.0 };
        if self.slots.is_empty() || (self.mix == 0.0 && target_mix == 0.0) {
            self.mix = target_mix;
            return;
        }
        debug_assert_eq!(samples.len() % channels, 0);
        self.ensure_states(channels);
        if self.mix == 0.0 {
            // Coming back from bypass: stale state would ring, start clean.
            for slot in &mut self.slots {
                slot.states.iter_mut().for_each(|s| *s = BiquadState::default());
            }
        }
        self.primed = true;

        let steady = self.mix == target_mix && self.slots.iter().all(|s| s.remaining == 0);
        if steady && target_mix == 1.0 {
            // Fast path: nothing is changing.
            for slot in self.slots.iter_mut() {
                let c = slot.cur;
                for frame in samples.chunks_exact_mut(channels) {
                    for (smp, st) in frame.iter_mut().zip(slot.states.iter_mut()) {
                        *smp = biquad_step(&c, st, *smp);
                    }
                }
            }
            return;
        }

        let mix_step = 1.0 / self.ramp_frames() as f32;
        let mut wet = vec![0.0f32; channels];
        for frame in samples.chunks_exact_mut(channels) {
            if self.mix != target_mix {
                self.mix = if target_mix > self.mix {
                    (self.mix + mix_step).min(target_mix)
                } else {
                    (self.mix - mix_step).max(target_mix)
                };
            }
            for slot in &mut self.slots {
                slot.advance();
            }
            for (ch, w) in wet.iter_mut().enumerate() {
                let mut v = frame[ch];
                for slot in &mut self.slots {
                    v = biquad_step(&slot.cur, &mut slot.states[ch], v);
                }
                *w = v;
            }
            let m = self.mix;
            for (smp, w) in frame.iter_mut().zip(wet.iter()) {
                *smp = if m >= 1.0 { *w } else { *smp + (*w - *smp) * m };
            }
        }
        // Bands that finished fading to a pass-through are done.
        self.slots.retain(|s| !(s.removing && s.remaining == 0));
    }
}

// ---------------------------------------------------------------------------
// Loudness normalization (EBU R128-style)
// ---------------------------------------------------------------------------

/// Default target: −14 LUFS (the common streaming target).
pub const DEFAULT_LOUDNESS_TARGET: f32 = -14.0;
/// Hard gain cap; exceeding it logs a warning instead of pumping.
/// Where the headroom guard starts to bend the signal (about -0.9 dBFS).
pub const GUARD_THRESHOLD: f32 = 0.9;

/// Headroom guard for the shared (DSP) path. EQ boosts and loudness gain
/// (up to +12 dB) can push peaks past full scale, and the output device
/// hard-clips anything over 1.0, which is harsh on loud transients. This
/// bends only what rises above [`GUARD_THRESHOLD`] along a smooth curve
/// that approaches, and never exceeds, 1.0. Everything below the threshold
/// is untouched, so ordinary material passes bit-for-bit.
pub fn headroom_guard(samples: &mut [f32]) {
    const T: f32 = GUARD_THRESHOLD;
    for s in samples.iter_mut() {
        let a = s.abs();
        if a > T {
            let over = (a - T) / (1.0 - T);
            *s = s.signum() * (T + (1.0 - T) * over.tanh());
        }
    }
}

pub const MAX_LOUDNESS_GAIN_DB: f32 = 12.0;
pub const MIN_LOUDNESS_GAIN_DB: f32 = -24.0;

/// Exact variant (documented, see module docs):
/// - K-weighting = high shelf (f0 = 1681.974 Hz, G = +3.9868 dB, Q = 0.70718,
///   RBJ Q-variant shelf) + high-pass (f0 = 60.4137 Hz, Q = 0.50033).
///   This matches the BS.1770-4 pre-filter/RLB corners; the Q-variant shelf
///   deviates ~0.4% from the standard's tabulated 48 kHz coefficients —
///   inaudible for a normalization gain, and rate-parameterized unlike the
///   fixed table.
/// - 400 ms blocks, 100 ms hop (75% overlap), zero-padded tail block.
/// - All channels weight 1.0 (v1 simplification: no 1.41 surround weight).
/// - Absolute gate −70 LUFS, relative gate −10 LU.
/// - Integrated = −0.691 + 10·log10(mean gated block energy).
fn k_weighting(sample_rate: u32) -> (Biquad, Biquad) {
    let pre = design_band(
        &EqBand {
            band_type: EqBandType::HighShelf,
            freq: 1681.974,
            gain_db: 3.9868,
            q: 0.70718,
        },
        sample_rate,
    );
    // RBJ high-pass with the RLB Q directly.
    let w0 = 2.0 * std::f64::consts::PI * 60.4137 / sample_rate as f64;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * 0.50033_f64);
    let a0 = 1.0 + alpha;
    let rlb = Biquad {
        b0: ((1.0 + cw) / 2.0) / a0,
        b1: (-(1.0 + cw)) / a0,
        b2: ((1.0 + cw) / 2.0) / a0,
        a1: (-2.0 * cw) / a0,
        a2: (1.0 - alpha) / a0,
    };
    (pre, rlb)
}

/// A running loudness meter: K-weighted mean square (BS.1770 pre-filter and
/// RLB high-pass, all channels weighted 1.0), smoothed over a few seconds.
/// Used to compare the level before and after a stage; the readings are
/// relative, not calibrated to a broadcast standard.
pub struct LoudnessMeter {
    sample_rate: u32,
    pre: Biquad,
    rlb: Biquad,
    pre_state: Vec<BiquadState>,
    rlb_state: Vec<BiquadState>,
    /// Smoothed K-weighted mean square (summed over channels).
    ms: f64,
    /// Seconds of (non-silent) audio integrated so far.
    seconds: f64,
}

/// Time constant of the smoothing, in seconds.
const METER_TAU_S: f64 = 1.5;
/// Chunks quieter than this (K-weighted, -70 LUFS) are silence: not integrated.
const METER_GATE_MS: f64 = 1.174e-7;

impl LoudnessMeter {
    pub fn new(sample_rate: u32) -> Self {
        let (pre, rlb) = k_weighting(sample_rate.max(8000));
        Self { sample_rate, pre, rlb, pre_state: Vec::new(), rlb_state: Vec::new(), ms: 0.0, seconds: 0.0 }
    }

    /// Re-tune for a new rate and forget everything.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        *self = Self::new(sample_rate);
    }

    pub fn reset(&mut self) {
        self.ms = 0.0;
        self.seconds = 0.0;
        self.pre_state.clear();
        self.rlb_state.clear();
    }

    /// Run the filters over a chunk and return its K-weighted mean square
    /// (summed over channels). The filter state always advances.
    pub fn measure(&mut self, samples: &[f32], channels: usize) -> f64 {
        if channels == 0 || samples.is_empty() {
            return 0.0;
        }
        if self.pre_state.len() != channels {
            self.pre_state = vec![BiquadState::default(); channels];
            self.rlb_state = vec![BiquadState::default(); channels];
        }
        let mut sum = vec![0.0f64; channels];
        for frame in samples.chunks_exact(channels) {
            for (ch, &x) in frame.iter().enumerate() {
                let y = biquad_step(&self.pre, &mut self.pre_state[ch], x);
                let z = biquad_step(&self.rlb, &mut self.rlb_state[ch], y) as f64;
                sum[ch] += z * z;
            }
        }
        let frames = (samples.len() / channels).max(1) as f64;
        sum.iter().map(|s| s / frames).sum()
    }

    /// Fold a chunk's mean square into the running value. Silence is skipped.
    pub fn integrate(&mut self, chunk_ms: f64, frames: usize) {
        if chunk_ms < METER_GATE_MS {
            return;
        }
        let dt = frames as f64 / self.sample_rate.max(1) as f64;
        if self.seconds == 0.0 {
            self.ms = chunk_ms;
        } else {
            let alpha = 1.0 - (-dt / METER_TAU_S).exp();
            self.ms += alpha * (chunk_ms - self.ms);
        }
        self.seconds += dt;
    }

    /// Smoothed loudness in LUFS-like units, once something has been integrated.
    pub fn lufs(&self) -> Option<f32> {
        (self.seconds > 0.0 && self.ms > 0.0).then(|| (-0.691 + 10.0 * self.ms.log10()) as f32)
    }

    /// Seconds of audio behind the reading.
    pub fn seconds(&self) -> f32 {
        self.seconds as f32
    }
}

/// Integrated loudness of interleaved f32 PCM, or `None` for silence
/// (every block below the absolute gate). Pure function — unit-testable.
pub fn integrated_lufs(frames: &[f32], channels: usize, sample_rate: u32) -> Option<f32> {
    if frames.is_empty() || channels == 0 || sample_rate == 0 {
        return None;
    }
    let (pre, rlb) = k_weighting(sample_rate);
    let mut pre_state = vec![BiquadState::default(); channels];
    let mut rlb_state = vec![BiquadState::default(); channels];

    let block_frames = (sample_rate as usize * 400) / 1000;
    let hop_frames = (sample_rate as usize * 100) / 1000;
    if block_frames == 0 || hop_frames == 0 {
        return None;
    }

    // Filter into a sliding window; drop frames older than the next block
    // start so a 24/192 album side never materializes in RAM.
    let mut window: Vec<f32> = Vec::new();
    let mut base: u64 = 0; // absolute frame index of window[0]
    let mut next_block: u64 = 0;
    let mut energies: Vec<f64> = Vec::new();

    let mut push_filtered = |chunk: &[f32], window: &mut Vec<f32>| {
        for (i, smp) in chunk.iter().enumerate() {
            let ch = i % channels;
            let y = biquad_step(&pre, &mut pre_state[ch], *smp);
            window.push(biquad_step(&rlb, &mut rlb_state[ch], y));
        }
    };

    for chunk in frames.chunks(4096 * channels) {
        push_filtered(chunk, &mut window);
        let available = base + (window.len() / channels) as u64;
        while next_block + block_frames as u64 <= available {
            let start = ((next_block - base) as usize) * channels;
            energies.push(block_energy(
                &window[start..start + block_frames * channels],
                channels,
            ));
            next_block += hop_frames as u64;
        }
        // Frames before the next block start can never be read again.
        let drop = ((next_block - base) as usize) * channels;
        if drop > 0 {
            window.drain(..drop.min(window.len()));
            base = next_block;
        }
    }
    // Tail: zero-padded partial blocks.
    let total = base + (window.len() / channels) as u64;
    while next_block < total {
        let start = ((next_block - base) as usize) * channels;
        let have = window.len().saturating_sub(start);
        let mut block = vec![0.0f32; block_frames * channels];
        let take = have.min(block.len());
        block[..take].copy_from_slice(&window[start..start + take]);
        energies.push(block_energy(&block, channels));
        next_block += hop_frames as u64;
    }

    gate_integrated(&energies)
}

/// Mean-square energy of one block, summed across channels.
fn block_energy(block: &[f32], channels: usize) -> f64 {
    let frames = (block.len() / channels).max(1) as f64;
    let mut z = 0.0f64;
    for ch in 0..channels {
        let mut ms = 0.0f64;
        for s in block.iter().skip(ch).step_by(channels) {
            ms += (*s as f64) * (*s as f64);
        }
        z += ms / frames;
    }
    z
}

/// Apply the absolute (−70 LUFS) and relative (−10 LU) gates.
fn gate_integrated(energies: &[f64]) -> Option<f32> {
    // Block loudness l = −0.691 + 10·log10(z); gate in the linear domain.
    let abs_gate = 10f64.powf((-70.0 + 0.691) / 10.0);
    let gated: Vec<f64> = energies.iter().copied().filter(|&z| z > abs_gate).collect();
    if gated.is_empty() {
        return None;
    }
    let mean = gated.iter().sum::<f64>() / gated.len() as f64;
    let integrated = -0.691 + 10.0 * mean.log10();
    let rel_gate = 10f64.powf((integrated - 10.0 + 0.691) / 10.0);
    let regated: Vec<f64> = gated.into_iter().filter(|&z| z > rel_gate).collect();
    if regated.is_empty() {
        return None;
    }
    let mean = regated.iter().sum::<f64>() / regated.len() as f64;
    Some((-0.691 + 10.0 * mean.log10()) as f32)
}

/// EBU R128-style loudness normalizer.
///
/// Honest design constraint: true track normalization needs the whole
/// track's loudness *before* playback. v1 does a fast pre-scan pass over
/// the deterministic transcode stream ([`scan_track_lufs`]) when a track
/// with no cached gain starts, then applies a static per-track gain.
/// Cost: double the LAN bandwidth and a second server transcode per
/// first-play of a track; the gain is cached in-memory keyed by
/// (track id, format) so repeats are free. DoP bypasses this entirely.
pub struct LoudnessNorm {
    enabled: bool,
    target_lufs: f32,
    cache: HashMap<(i64, StreamFormat), f32>,
    /// (integrated LUFS, sample peak) per track, so the gain can be re-planned
    /// when the EQ changes without another pre-scan.
    levels: HashMap<(i64, StreamFormat), (f32, f32)>,
}

/// Headroom kept below full scale when planning gain (dB).
pub const HEADROOM_MARGIN_DB: f32 = 1.0;

/// The loudness gain (dB) that reaches the target *without* the track's peak,
/// after `eq_boost_db` of EQ boost, passing full scale. The output device
/// hard-clips anything over 1.0, which is harsh; a quieter result beats that.
pub fn plan_gain_db(wanted_db: f32, peak: f32, eq_boost_db: f32) -> f32 {
    if peak <= 1e-6 {
        return wanted_db;
    }
    let room = -20.0 * peak.log10() - eq_boost_db - HEADROOM_MARGIN_DB;
    wanted_db.min(room).max(MIN_LOUDNESS_GAIN_DB)
}

impl LoudnessNorm {
    pub fn new(target_lufs: f32) -> Self {
        Self {
            enabled: false,
            target_lufs,
            cache: HashMap::new(),
            levels: HashMap::new(),
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_target(&mut self, lufs: f32) {
        if lufs.is_finite() {
            self.target_lufs = lufs.clamp(-40.0, 0.0);
        }
    }

    pub fn target(&self) -> f32 {
        self.target_lufs
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Gain in dB for this track: cached, else `scan()` once and cache.
    /// A failed or silent scan yields 0 dB (never blocks playback).
    /// The gain for a track, planned against its real peak and the EQ's
    /// worst-case boost so the result cannot clip. `scan` runs at most once
    /// per (track, format); the levels are cached, the gain is re-planned.
    pub fn gain_for_levels(
        &mut self,
        track_id: i64,
        fmt: StreamFormat,
        eq_boost_db: f32,
        scan: impl FnOnce() -> Result<Option<(f32, f32)>, MusicError>,
    ) -> f32 {
        let key = (track_id, fmt);
        let (lufs, peak) = match self.levels.get(&key) {
            Some(&l) => l,
            None => match scan() {
                Ok(Some(l)) => {
                    self.levels.insert(key, l);
                    l
                }
                Ok(None) => return 0.0, // silence: nothing to normalize
                Err(e) => {
                    tracing::warn!(track_id, "loudness pre-scan failed ({e}); playing unnormalized");
                    return 0.0;
                }
            },
        };
        let wanted = (self.target_lufs - lufs).clamp(MIN_LOUDNESS_GAIN_DB, MAX_LOUDNESS_GAIN_DB);
        let planned = plan_gain_db(wanted, peak, eq_boost_db);
        if planned < wanted {
            tracing::info!(track_id, wanted, planned, "loudness gain reduced to keep the peak below full scale");
        }
        planned
    }

    pub fn gain_for(
        &mut self,
        track_id: i64,
        fmt: StreamFormat,
        scan: impl FnOnce() -> Result<Option<f32>, MusicError>,
    ) -> f32 {
        if let Some(&g) = self.cache.get(&(track_id, fmt)) {
            return g;
        }
        let gain = match scan() {
            Ok(Some(lufs)) => {
                let raw = self.target_lufs - lufs;
                if raw > MAX_LOUDNESS_GAIN_DB {
                    tracing::warn!(
                        track_id,
                        integrated = lufs,
                        "loudness gain capped at +{MAX_LOUDNESS_GAIN_DB} dB"
                    );
                }
                raw.clamp(MIN_LOUDNESS_GAIN_DB, MAX_LOUDNESS_GAIN_DB)
            }
            Ok(None) => 0.0, // silence: nothing to normalize
            Err(e) => {
                tracing::warn!(
                    track_id,
                    "loudness pre-scan failed ({e}); playing unnormalized"
                );
                0.0
            }
        };
        self.cache.insert((track_id, fmt), gain);
        gain
    }
}

/// Pre-scan pass: open a second stream for the full track, decode it,
/// and measure integrated loudness. Deterministic for transcodes (the
/// server renders the same bytes), so the measured gain applies to the
/// playback stream. Doubles LAN bandwidth per first-play — the documented
/// v1 cost.
pub fn scan_track_lufs(
    transport: &dyn Transport,
    track_id: i64,
    fmt: StreamFormat,
) -> Result<Option<f32>, MusicError> {
    Ok(scan_track_levels(transport, track_id, fmt)?.map(|(lufs, _peak)| lufs))
}

/// Like [`scan_track_lufs`], but also returns the track's sample peak
/// (linear, 1.0 = full scale) from the same pass, so the gain can be planned
/// to keep that peak, after any EQ boost, below full scale.
pub fn scan_track_levels(
    transport: &dyn Transport,
    track_id: i64,
    fmt: StreamFormat,
) -> Result<Option<(f32, f32)>, MusicError> {
    let opts = StreamOptions {
        format: Some(fmt),
        ..Default::default()
    };
    let info = transport.open_stream(track_id, &opts)?;
    let mut decoder = StreamDecoder::new(info.reader, false)?;
    let spec = decoder.spec();
    if spec.sample_rate == 0 || spec.channels == 0 {
        return Ok(None);
    }
    let channels = spec.channels as usize;

    // Incremental: K-filter + block energies without materializing the
    // whole track (mirrors `integrated_lufs`' streaming core).
    let (pre, rlb) = k_weighting(spec.sample_rate);
    let mut pre_state = vec![BiquadState::default(); channels];
    let mut rlb_state = vec![BiquadState::default(); channels];
    let block_frames = (spec.sample_rate as usize * 400) / 1000;
    let hop_frames = (spec.sample_rate as usize * 100) / 1000;
    let mut window: Vec<f32> = Vec::new();
    let mut base: u64 = 0;
    let mut next_block: u64 = 0;
    let mut energies: Vec<f64> = Vec::new();
    let mut pcm = vec![0.0f32; 4096 * channels];
    let mut peak = 0.0f32;

    loop {
        let n = decoder.decode_interleaved(&mut pcm)?;
        if n == 0 {
            break;
        }
        for (i, smp) in pcm[..n * channels].iter().enumerate() {
            peak = peak.max(smp.abs());
            let ch = i % channels;
            let y = biquad_step(&pre, &mut pre_state[ch], *smp);
            window.push(biquad_step(&rlb, &mut rlb_state[ch], y));
        }
        let available = base + (window.len() / channels) as u64;
        while next_block + block_frames as u64 <= available {
            let start = ((next_block - base) as usize) * channels;
            energies.push(block_energy(
                &window[start..start + block_frames * channels],
                channels,
            ));
            next_block += hop_frames as u64;
        }
        let drop = ((next_block - base) as usize) * channels;
        if drop > 0 {
            window.drain(..drop.min(window.len()));
            base = next_block;
        }
    }
    let total = base + (window.len() / channels) as u64;
    while next_block < total {
        let start = ((next_block - base) as usize) * channels;
        let have = window.len().saturating_sub(start);
        let mut block = vec![0.0f32; block_frames * channels];
        let take = have.min(block.len());
        block[..take].copy_from_slice(&window[start..start + take]);
        energies.push(block_energy(&block, channels));
        next_block += hop_frames as u64;
    }
    Ok(gate_integrated(&energies).map(|lufs| (lufs, peak)))
}

// ---------------------------------------------------------------------------
// Gain ramp (click-free gain changes on track boundaries)
// ---------------------------------------------------------------------------

/// Linear ramp between gains, applied per sample. The engine retargets it
/// on every track open; consecutive tracks with different loudness gains
/// crossfade gains (not audio) over ~50 ms — no clicks, no pumping.
pub struct GainRamp {
    current: f32,
    target: f32,
    step: f32,
    ramp_frames: usize,
}

impl GainRamp {
    pub fn new(ramp_frames: usize) -> Self {
        Self {
            current: 1.0,
            target: 1.0,
            step: 0.0,
            ramp_frames: ramp_frames.max(1),
        }
    }

    pub fn retarget(&mut self, target_db: f32) {
        let target = 10f32.powf(target_db / 20.0);
        self.step = (target - self.current) / self.ramp_frames as f32;
        self.target = target;
    }

    /// Target gain in dB (for tests).
    #[cfg(test)]
    fn target_db(&self) -> f32 {
        20.0 * self.target.log10()
    }

    pub fn apply(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            if (self.current - self.target).abs() > f32::EPSILON {
                self.current += self.step;
                // Clamp overshoot from float accumulation.
                if (self.step > 0.0 && self.current > self.target)
                    || (self.step < 0.0 && self.current < self.target)
                {
                    self.current = self.target;
                }
            }
            *s *= self.current;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sine(freq: f32, frames: usize, rate: u32, amp: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * amp)
            .collect()
    }

    fn stereo(mono: &[f32]) -> Vec<f32> {
        mono.iter().flat_map(|&s| [s, s]).collect()
    }

    fn rms_steady(samples: &[f32], skip: usize) -> f32 {
        let tail = &samples[skip.min(samples.len())..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len().max(1) as f32).sqrt()
    }

    #[test]
    fn bypass_is_bit_transparent() {
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }])
        .unwrap();
        eq.set_enabled(false);
        let input = stereo(&sine(440.0, 8192, 44100, 0.5));
        let mut out = input.clone();
        eq.process(&mut out, 2);
        assert_eq!(out, input, "disabled EQ must not touch a single sample");
    }

    #[test]
    fn empty_chain_is_transparent() {
        let mut eq = ParametricEq::new(48000);
        let input = stereo(&sine(440.0, 4096, 48000, 0.5));
        let mut out = input.clone();
        eq.process(&mut out, 2);
        assert_eq!(out, input);
    }

    #[test]
    fn low_frequency_bands_stay_accurate_at_192k() {
        // A 100 Hz low shelf at 192 kHz needs coefficients ~1e-3 from 1.0;
        // f32 coefficients lose accuracy here. +12 dB well below the corner must be x3.98.
        let mut eq = ParametricEq::new(192_000);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::LowShelf,
            freq: 100.0,
            gain_db: 12.0,
            q: 0.7,
        }])
        .unwrap();
        let mut out = stereo(&sine(15.0, 192_000 * 4, 192_000, 0.1));
        eq.process(&mut out, 2);
        let ratio = rms_steady(&out, 192_000 * 2 * 2) / (0.1 * std::f32::consts::FRAC_1_SQRT_2);
        assert!((ratio - 3.98).abs() < 0.12, "expected ~x3.98 at 15 Hz, got {ratio}");
    }

    #[test]
    fn band_above_the_usable_range_is_designed_at_the_cap_and_stays_stable() {
        let rate = 44_100;
        let cap = usable_freq(24_000.0, rate);
        assert!(cap < 22_050.0 * 0.95, "capped below Nyquist, got {cap}");
        let band = |f| EqBand { band_type: EqBandType::HighShelf, freq: f, gain_db: 6.0, q: 0.7 };
        let input = stereo(&sine(5000.0, 8192, rate, 0.5));
        let (mut a, mut b) = (input.clone(), input);
        let mut eq_hi = ParametricEq::new(rate);
        eq_hi.set_bands(vec![band(24_000.0)]).unwrap();
        eq_hi.process(&mut a, 2);
        let mut eq_cap = ParametricEq::new(rate);
        eq_cap.set_bands(vec![band(cap)]).unwrap();
        eq_cap.process(&mut b, 2);
        assert!(a.iter().all(|s| s.is_finite() && s.abs() < 4.0));
        assert_eq!(a, b, "an out-of-range frequency behaves exactly like the cap");
        assert_eq!(usable_freq(1000.0, rate), 1000.0, "in-range bands are untouched");
    }

    fn peak_band(gain_db: f32) -> EqBand {
        EqBand { band_type: EqBandType::Peaking, freq: 1000.0, gain_db, q: 1.0 }
    }

    /// Largest jump between neighbouring samples of a channel.
    fn max_step(interleaved: &[f32], channels: usize) -> f32 {
        interleaved
            .iter()
            .step_by(channels)
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn live_band_changes_glide_instead_of_clicking() {
        // A low shelf on a bass tone: resetting the filter state or jumping
        // the coefficients here throws the waveform by a large fraction of
        // its amplitude, while the tone itself moves only ~0.01 per sample.
        let rate = 44100;
        let shelf = |gain_db| EqBand { band_type: EqBandType::LowShelf, freq: 150.0, gain_db, q: 0.7 };
        let mut eq = ParametricEq::new(rate);
        eq.set_bands(vec![shelf(2.0)]).unwrap();
        let tone = stereo(&sine(60.0, rate as usize * 2, rate, 0.3));
        let split = (rate as usize / 2 + 184) * 2; // near a waveform peak
        let (first, second) = tone.split_at(split);
        let mut a = first.to_vec();
        eq.process(&mut a, 2); // audio is flowing now
        eq.set_bands(vec![shelf(12.0)]).unwrap(); // drag: +2 dB -> +12 dB
        let mut b = second.to_vec();
        eq.process(&mut b, 2);
        // Measure across the seam, where the change happened.
        let all = [a.clone(), b.clone()].concat();
        let step = max_step(&all[2000..], 2);
        assert!(step < 0.02, "no click while the band changes, biggest step {step}");
        // ...and it lands where a filter that was +12 dB all along would.
        let mut reference = ParametricEq::new(rate);
        reference.set_bands(vec![shelf(12.0)]).unwrap();
        let mut steady = tone.clone();
        reference.process(&mut steady, 2);
        let tail = rms_steady(&b, b.len() - 4410 * 2);
        let want = rms_steady(&steady, steady.len() - 4410 * 2);
        assert!((tail - want).abs() / want < 0.02, "settles at +12 dB: {tail} vs {want}");
    }

    #[test]
    fn adding_and_removing_bands_live_fades_without_a_click() {
        let rate = 44100;
        let mut eq = ParametricEq::new(rate);
        let tone = stereo(&sine(1000.0, rate as usize * 2, rate, 0.3));
        let chunk = tone.len() / 4;
        let mut out = tone[..chunk].to_vec();
        eq.process(&mut out, 2); // flat, flowing
        eq.set_bands(vec![peak_band(6.0)]).unwrap(); // add
        let mut added = tone[chunk..chunk * 2].to_vec();
        eq.process(&mut added, 2);
        assert!(max_step(&[out.clone(), added.clone()].concat(), 2) < 0.1, "adding a band is smooth");
        assert!(rms_steady(&added, added.len() - 4410) > 0.3 * 1.8 * 0.70, "the added band is audible");
        eq.set_bands(vec![]).unwrap(); // remove
        let mut removed = tone[chunk * 2..chunk * 3].to_vec();
        let src = removed.clone();
        eq.process(&mut removed, 2);
        assert!(max_step(&[added.clone(), removed.clone()].concat(), 2) < 0.1, "removing a band is smooth");
        let n = removed.len() - 4410;
        let err = removed[n..].iter().zip(&src[n..]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-3, "back to the untouched signal, error {err}");
        let mut again = tone[chunk * 3..].to_vec();
        let src = again.clone();
        eq.process(&mut again, 2);
        assert_eq!(again, src, "an empty chain is bit-transparent again");
    }

    #[test]
    fn toggling_the_eq_live_fades_and_then_is_bit_transparent() {
        let rate = 44100;
        let mut eq = ParametricEq::new(rate);
        eq.set_bands(vec![peak_band(12.0)]).unwrap();
        let tone = stereo(&sine(1000.0, rate as usize * 2, rate, 0.3));
        let chunk = tone.len() / 4;
        let mut warm = tone[..chunk].to_vec();
        eq.process(&mut warm, 2);
        eq.set_enabled(false);
        let mut fade = tone[chunk..chunk * 2].to_vec();
        eq.process(&mut fade, 2);
        assert!(max_step(&[warm.clone(), fade.clone()].concat(), 2) < 0.2, "switching off is smooth");
        let n = fade.len() - 2000;
        assert_eq!(&fade[n..], &tone[chunk..chunk * 2][n..], "faded all the way to the dry signal");
        let mut off = tone[chunk * 2..chunk * 3].to_vec();
        let src = off.clone();
        eq.process(&mut off, 2);
        assert_eq!(off, src, "bit-transparent once faded out");
        // Switching back on fades in, without ringing from stale state.
        eq.set_enabled(true);
        let mut on = tone[chunk * 3..].to_vec();
        eq.process(&mut on, 2);
        assert!(max_step(&[off.clone(), on.clone()].concat(), 2) < 0.2, "switching on is smooth");
    }

    #[test]
    fn changes_before_any_audio_apply_at_once() {
        // A new track (or rate) has nothing playing, so there is nothing to fade.
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![peak_band(12.0)]).unwrap();
        eq.set_enabled(false);
        let input = stereo(&sine(1000.0, 2048, 44100, 0.3));
        let mut out = input.clone();
        eq.process(&mut out, 2);
        assert_eq!(out, input, "disabled before the first sample: untouched from sample one");
    }

    #[test]
    fn loudness_meter_follows_level_and_ignores_silence() {
        let rate = 44_100;
        let mut m = LoudnessMeter::new(rate);
        assert!(m.lufs().is_none(), "nothing measured yet");
        let tone = |amp: f32| stereo(&sine(1000.0, rate as usize * 2, rate, amp));
        let read = |amp: f32| {
            let mut m = LoudnessMeter::new(rate);
            for c in tone(amp).chunks(4096 * 2) {
                let ms = m.measure(c, 2);
                m.integrate(ms, c.len() / 2);
            }
            m.lufs().unwrap()
        };
        let (a, b, c) = (read(0.05), read(0.1), read(0.4));
        assert!(((b - a) - 6.02).abs() < 0.1, "double the amplitude is +6 dB: {}", b - a);
        assert!(((c - b) - 12.04).abs() < 0.1, "four times is +12 dB: {}", c - b);
        assert!((-30.0..-10.0).contains(&b), "a plausible LUFS reading: {b}");
        // Silence is not integrated: the reading holds.
        let before = read(0.1);
        let mut m2 = LoudnessMeter::new(rate);
        for c in tone(0.1).chunks(4096 * 2) {
            let ms = m2.measure(c, 2);
            m2.integrate(ms, c.len() / 2);
        }
        let silence = vec![0.0f32; 4096 * 2];
        for _ in 0..20 {
            let ms = m2.measure(&silence, 2);
            m2.integrate(ms, 4096);
        }
        assert!((m2.lufs().unwrap() - before).abs() < 0.3, "silence does not pull the reading down ({} vs {before})", m2.lufs().unwrap());
        assert!(m2.seconds() > 1.5);
        // K-weighting: the same amplitude reads lower at 60 Hz than at 1 kHz (the high-pass), higher at 8 kHz (the shelf).
        let at = |hz: f32| {
            let mut m = LoudnessMeter::new(rate);
            for c in stereo(&sine(hz, rate as usize * 2, rate, 0.1)).chunks(4096 * 2) {
                let ms = m.measure(c, 2);
                m.integrate(ms, c.len() / 2);
            }
            m.lufs().unwrap()
        };
        assert!(at(60.0) < at(1000.0) - 1.0 && at(8000.0) > at(1000.0) + 1.0);
        m.reset();
        assert!(m.lufs().is_none());
    }

    #[test]
    fn peaking_boosts_in_band_only() {
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }])
        .unwrap();

        // In-band: 1 kHz should roughly double in amplitude (+6 dB).
        let mut in_band = stereo(&sine(1000.0, 44100, 44100, 0.4));
        eq.process(&mut in_band, 2);
        let ratio = rms_steady(&in_band, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (1.9..2.1).contains(&ratio),
            "expected ~2x amplitude at 1 kHz, got {ratio}"
        );

        // Out of band: 100 Hz must be (nearly) untouched by a 1 kHz Q=1 bell.
        let mut eq2 = ParametricEq::new(44100);
        eq2.set_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }])
        .unwrap();
        let mut low = stereo(&sine(100.0, 44100, 44100, 0.4));
        eq2.process(&mut low, 2);
        let low_ratio = rms_steady(&low, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (0.97..1.03).contains(&low_ratio),
            "100 Hz should be untouched, got {low_ratio}"
        );
    }

    #[test]
    fn low_shelf_boosts_lows_not_highs() {
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::LowShelf,
            freq: 200.0,
            gain_db: 6.0,
            q: 0.7,
        }])
        .unwrap();
        let mut low = stereo(&sine(50.0, 44100, 44100, 0.4));
        eq.process(&mut low, 2);
        let low_ratio = rms_steady(&low, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (1.9..2.1).contains(&low_ratio),
            "50 Hz shelf boost: {low_ratio}"
        );

        let mut eq2 = ParametricEq::new(44100);
        eq2.set_bands(vec![EqBand {
            band_type: EqBandType::LowShelf,
            freq: 200.0,
            gain_db: 6.0,
            q: 0.7,
        }])
        .unwrap();
        let mut high = stereo(&sine(8000.0, 44100, 44100, 0.4));
        eq2.process(&mut high, 2);
        let high_ratio = rms_steady(&high, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (0.97..1.03).contains(&high_ratio),
            "8 kHz untouched: {high_ratio}"
        );
    }

    #[test]
    fn band_validation_rejects_garbage() {
        assert!(validate_bands(
            &[EqBand {
                band_type: EqBandType::Peaking,
                freq: 1000.0,
                gain_db: 0.0,
                q: 1.0,
            }; 9]
        )
        .is_err());
        assert!(validate_bands(&[EqBand {
            band_type: EqBandType::Peaking,
            freq: 5.0,
            gain_db: 0.0,
            q: 1.0,
        }])
        .is_err());
        assert!(validate_bands(&[EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 0.0,
            q: 0.0,
        }])
        .is_err());
        assert!(validate_bands(&[EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }])
        .is_ok());
    }

    #[test]
    fn loudness_silence_is_none() {
        let frames = vec![0.0f32; 44100 * 2];
        assert_eq!(integrated_lufs(&frames, 2, 44100), None);
    }

    #[test]
    fn loudness_6db_apart_measures_6db() {
        // Two seconds of 1 kHz sine, 6 dB apart in amplitude.
        let a = stereo(&sine(1000.0, 88200, 44100, 0.5));
        let b = stereo(&sine(1000.0, 88200, 44100, 0.25));
        let la = integrated_lufs(&a, 2, 44100).expect("loud A");
        let lb = integrated_lufs(&b, 2, 44100).expect("loud B");
        assert!(
            (la - lb - 6.0).abs() < 0.15,
            "expected 6 dB apart, got {la} vs {lb}"
        );
    }

    #[test]
    fn loudness_gain_math_and_cap() {
        let mut norm = LoudnessNorm::new(-14.0);
        // Integrated −20 LUFS → +6 dB toward −14.
        let g = norm.gain_for(1, StreamFormat::Flac, || Ok(Some(-20.0)));
        assert!((g - 6.0).abs() < 1e-6, "gain {g}");
        // Cached: the scan must not run again.
        let calls = std::cell::Cell::new(0);
        let g2 = norm.gain_for(1, StreamFormat::Flac, || {
            calls.set(calls.get() + 1);
            Ok(Some(-20.0))
        });
        assert_eq!(g2, g);
        assert_eq!(calls.get(), 0);
        // Integrated −40 LUFS → raw +26 dB, capped at +12.
        let g3 = norm.gain_for(2, StreamFormat::Flac, || Ok(Some(-40.0)));
        assert!((g3 - 12.0).abs() < 1e-6, "capped gain {g3}");
        // Hot master: −8 LUFS → −6 dB.
        let g4 = norm.gain_for(3, StreamFormat::Flac, || Ok(Some(-8.0)));
        assert!((g4 + 6.0).abs() < 1e-6, "cut gain {g4}");
        // Silence → 0 dB.
        let g5 = norm.gain_for(4, StreamFormat::Flac, || Ok(None));
        assert_eq!(g5, 0.0);
        // Scan failure → 0 dB, playback never blocked.
        let g6 = norm.gain_for(5, StreamFormat::Flac, || {
            Err::<Option<f32>, _>(MusicError::Http("down".into()))
        });
        assert_eq!(g6, 0.0);
    }

    #[test]
    fn gain_ramp_reaches_target_without_click() {
        let mut ramp = GainRamp::new(100);
        ramp.retarget(6.0); // +6 dB ≈ ×1.995
        assert!((ramp.target_db() - 6.0).abs() < 1e-4);
        let mut buf = vec![1.0f32; 200];
        ramp.apply(&mut buf);
        // First sample ramps from 1.0, last samples sit at the target.
        assert!(buf[0] > 1.0 && buf[0] < 1.995);
        assert!((buf[199] - 1.995).abs() < 0.01);
        // Monotonic: no discontinuities.
        for w in buf.windows(2) {
            assert!(w[1] >= w[0] - 1e-6);
        }
    }

    /// WAV fixture for the pre-scan test: 16-bit stereo.
    fn wav_bytes(frames: &[f32]) -> Vec<u8> {
        let data: Vec<i16> = frames
            .iter()
            .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data.len() as u32 * 2).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&44100u32.to_le_bytes());
        v.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32 * 2).to_le_bytes());
        for s in data {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    struct WavTransport {
        body: Vec<u8>,
    }

    impl Transport for WavTransport {
        fn open_stream(
            &self,
            _track_id: i64,
            _opts: &StreamOptions,
        ) -> Result<crate::transport::StreamInfo, MusicError> {
            Ok(crate::transport::StreamInfo {
                progress: None,
                reader: Box::new(Cursor::new(self.body.clone())),
                content_type: "audio/wav".into(),
                chain: None,
                gapless_next: None,
                gapless_mode: None,
            })
        }
    }

    #[test]
    fn prescan_measures_stream_loudness() {
        let frames = stereo(&sine(1000.0, 88200, 44100, 0.5));
        let t = WavTransport {
            body: wav_bytes(&frames),
        };
        let measured = scan_track_lufs(&t, 7, StreamFormat::Flac).expect("scan");
        let direct = integrated_lufs(&frames, 2, 44100).expect("direct");
        let m = measured.expect("not silence");
        // 16-bit quantization + decode round-trip: agree within 0.2 LU.
        assert!((m - direct).abs() < 0.2, "scan {m} vs direct {direct}");
    }

    #[test]
    fn headroom_guard_leaves_normal_material_bit_exact() {
        let mut v: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.37).sin() * GUARD_THRESHOLD).collect();
        let before = v.clone();
        headroom_guard(&mut v);
        assert_eq!(v, before, "nothing at or below the threshold changes");
    }

    #[test]
    fn headroom_guard_never_exceeds_full_scale_and_keeps_order() {
        let mut prev = 0.0f32;
        for i in 0..=4000 {
            let x = i as f32 * 0.001; // 0.0 ..= 4.0
            let mut a = [x, -x];
            headroom_guard(&mut a);
            assert!(a[0] <= 1.0 && a[1] >= -1.0, "{x} -> {}", a[0]);
            assert_eq!(a[0], -a[1], "symmetric");
            assert!(a[0] >= prev, "monotonic at {x}");
            prev = a[0];
        }
    }

    #[test]
    fn headroom_guard_is_continuous_at_the_threshold() {
        let mut a = [GUARD_THRESHOLD + 1e-4];
        headroom_guard(&mut a);
        assert!((a[0] - (GUARD_THRESHOLD + 1e-4)).abs() < 2e-4, "no step where it engages");
    }

    #[test]
    fn headroom_guard_tames_the_measured_overs() {
        // A 1.37 FS peak (track 2 of the reported album) lands just under 1.0.
        let mut a = [1.37f32, -1.06];
        headroom_guard(&mut a);
        assert!(a[0] > 0.99 && a[0] <= 1.0, "{}", a[0]);
        assert!(a[1] < -0.95 && a[1] >= -1.0, "{}", a[1]);
    }

    #[test]
    fn max_boost_of_no_bands_or_cuts_is_zero() {
        assert_eq!(max_boost_db(&[], 44_100), 0.0);
        let cut = EqBand { band_type: EqBandType::Peaking, freq: 1000.0, gain_db: -6.0, q: 1.0 };
        assert_eq!(max_boost_db(&[cut], 44_100), 0.0, "a cut never raises the peak");
    }

    #[test]
    fn max_boost_finds_the_peak_of_a_band() {
        let b = EqBand { band_type: EqBandType::Peaking, freq: 1000.0, gain_db: 6.0, q: 1.0 };
        let m = max_boost_db(&[b], 44_100);
        assert!((m - 6.0).abs() < 0.2, "a +6 dB peaking band boosts about 6 dB, got {m}");
    }

    #[test]
    fn max_boost_of_several_bands_is_the_true_worst_case_not_their_sum() {
        // The reported EQ: low shelf +2.5, peaks +1.5 and +1.5, high shelf +2.
        let bands = [
            EqBand { band_type: EqBandType::LowShelf, freq: 100.0, gain_db: 2.5, q: 0.7 },
            EqBand { band_type: EqBandType::Peaking, freq: 250.0, gain_db: 1.5, q: 1.0 },
            EqBand { band_type: EqBandType::Peaking, freq: 3000.0, gain_db: 1.5, q: 1.0 },
            EqBand { band_type: EqBandType::HighShelf, freq: 10_000.0, gain_db: 2.0, q: 0.7 },
        ];
        let m = max_boost_db(&bands, 44_100);
        assert!(m > 2.0 && m < 4.0, "somewhere near the strongest band, well under the sum of all four: {m}");
    }

    #[test]
    fn planned_gain_keeps_the_peak_below_full_scale() {
        // Wants +5.3 dB, but the track peaks at 0.70 FS (-3.1 dB) and the EQ adds 3.4 dB.
        let g = plan_gain_db(5.3, 0.70, 3.4);
        let out_peak = 0.70 * 10f32.powf((g + 3.4) / 20.0);
        assert!(out_peak <= 10f32.powf(-HEADROOM_MARGIN_DB / 20.0) + 1e-4, "{out_peak}");
        assert!(g < 5.3, "reduced from what the target asked for");
    }

    #[test]
    fn planned_gain_is_untouched_when_there_is_room() {
        assert_eq!(plan_gain_db(2.0, 0.3, 0.0), 2.0);
        assert_eq!(plan_gain_db(-4.0, 0.9, 3.0), -4.0, "attenuation is never held back");
    }
}
