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

/// Normalized biquad coefficients (a0 = 1).
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

/// Direct Form II transposed state, one per channel.
#[derive(Debug, Clone, Copy, Default)]
struct BiquadState {
    z1: f32,
    z2: f32,
}

#[inline]
fn biquad_step(c: &Biquad, s: &mut BiquadState, x: f32) -> f32 {
    let y = c.b0 * x + s.z1;
    s.z1 = c.b1 * x - c.a1 * y + s.z2;
    s.z2 = c.b2 * x - c.a2 * y;
    y
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

/// Design one band at `sample_rate` using the RBJ cookbook.
fn design_band(band: &EqBand, sample_rate: u32) -> Biquad {
    let a = 10f32.powf(band.gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * band.freq / sample_rate as f32;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * band.q);

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
            let s = band.q.clamp(0.1, 3.0);
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
            let s = band.q.clamp(0.1, 3.0);
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

/// Up-to-8-band parametric EQ over interleaved f32.
///
/// Bypass is bit-transparent: when disabled (or with no bands) `process`
/// does not touch the buffer at all.
pub struct ParametricEq {
    bands: Vec<EqBand>,
    enabled: bool,
    sample_rate: u32,
    coeffs: Vec<Biquad>,
    /// states[band][channel]
    states: Vec<Vec<BiquadState>>,
}

impl ParametricEq {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            bands: Vec::new(),
            enabled: true,
            sample_rate,
            coeffs: Vec::new(),
            states: Vec::new(),
        }
    }

    pub fn set_bands(&mut self, bands: Vec<EqBand>) -> Result<(), MusicError> {
        validate_bands(&bands)?;
        self.bands = bands;
        self.redesign();
        Ok(())
    }

    pub fn bands(&self) -> &[EqBand] {
        &self.bands
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Redesign filters when the stream rate changes; state is cleared
    /// (a rate change is a track boundary, never mid-track).
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.redesign();
        }
    }

    fn redesign(&mut self) {
        self.coeffs = self
            .bands
            .iter()
            .map(|b| design_band(b, self.sample_rate))
            .collect();
        self.states.clear();
    }

    fn ensure_states(&mut self, channels: usize) {
        if self.states.len() != self.coeffs.len()
            || self.states.first().map(|s| s.len()) != Some(channels)
        {
            self.states = self
                .coeffs
                .iter()
                .map(|_| vec![BiquadState::default(); channels])
                .collect();
        }
    }

    /// Process interleaved samples in place. Bit-transparent no-op unless
    /// enabled with at least one band.
    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        if !self.enabled || self.coeffs.is_empty() || channels == 0 {
            return;
        }
        debug_assert_eq!(samples.len() % channels, 0);
        self.ensure_states(channels);
        for (coeff, states) in self.coeffs.iter().zip(self.states.iter_mut()) {
            for frame in samples.chunks_exact_mut(channels) {
                for (smp, st) in frame.iter_mut().zip(states.iter_mut()) {
                    *smp = biquad_step(coeff, st, *smp);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Loudness normalization (EBU R128-style)
// ---------------------------------------------------------------------------

/// Default target: −14 LUFS (the common streaming target).
pub const DEFAULT_LOUDNESS_TARGET: f32 = -14.0;
/// Hard gain cap; exceeding it logs a warning instead of pumping.
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
    let w0 = 2.0 * std::f32::consts::PI * 60.4137 / sample_rate as f32;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * 0.50033);
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
}

impl LoudnessNorm {
    pub fn new(target_lufs: f32) -> Self {
        Self {
            enabled: false,
            target_lufs,
            cache: HashMap::new(),
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

    loop {
        let n = decoder.decode_interleaved(&mut pcm)?;
        if n == 0 {
            break;
        }
        for (i, smp) in pcm[..n * channels].iter().enumerate() {
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
    Ok(gate_integrated(&energies))
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
}
