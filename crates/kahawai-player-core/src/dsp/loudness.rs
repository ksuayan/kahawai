//! Loudness normalization (EBU R128-style): the K-weighted meter,
//! integrated loudness, gain planning, and the per-track pre-scan.

use std::collections::{HashMap, HashSet};

use kahawai_core::{api::StreamFormat, MusicError};

use super::biquad::{biquad_step, Biquad, BiquadState};
use super::eq::{design_band, EqBand, EqBandType};
use crate::decode::StreamDecoder;
use crate::transport::{StreamOptions, Transport};

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
        Self {
            sample_rate,
            pre,
            rlb,
            pre_state: Vec::new(),
            rlb_state: Vec::new(),
            ms: 0.0,
            seconds: 0.0,
        }
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
    /// Pre-scans still running (possibly behind a track already playing
    /// unnormalized), so the same track is not scanned twice at once.
    scanning: HashSet<(i64, StreamFormat)>,
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
            scanning: HashSet::new(),
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
    /// Whether this (track, format)'s levels are already cached, so no
    /// pre-scan is needed.
    pub fn has_levels(&self, track_id: i64, fmt: StreamFormat) -> bool {
        self.levels.contains_key(&(track_id, fmt))
    }

    /// Whether a pre-scan of this (track, format) is still running.
    pub fn is_scanning(&self, track_id: i64, fmt: StreamFormat) -> bool {
        self.scanning.contains(&(track_id, fmt))
    }

    /// A pre-scan of this (track, format) has started.
    pub fn scan_started(&mut self, track_id: i64, fmt: StreamFormat) {
        self.scanning.insert((track_id, fmt));
    }

    /// A pre-scan finished: keep its levels (`None`: silent or failed, so the
    /// next play may try again).
    pub fn scan_finished(&mut self, track_id: i64, fmt: StreamFormat, levels: Option<(f32, f32)>) {
        self.scanning.remove(&(track_id, fmt));
        if let Some(l) = levels {
            self.levels.insert((track_id, fmt), l);
        }
    }

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
                    tracing::warn!(
                        track_id,
                        "loudness pre-scan failed ({e}); playing unnormalized"
                    );
                    return 0.0;
                }
            },
        };
        let wanted = (self.target_lufs - lufs).clamp(MIN_LOUDNESS_GAIN_DB, MAX_LOUDNESS_GAIN_DB);
        let planned = plan_gain_db(wanted, peak, eq_boost_db);
        if planned < wanted {
            tracing::info!(
                track_id,
                wanted,
                planned,
                "loudness gain reduced to keep the peak below full scale"
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::test_util::{sine, stereo};
    use std::io::Cursor;

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
        assert!(
            ((b - a) - 6.02).abs() < 0.1,
            "double the amplitude is +6 dB: {}",
            b - a
        );
        assert!(
            ((c - b) - 12.04).abs() < 0.1,
            "four times is +12 dB: {}",
            c - b
        );
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
        assert!(
            (m2.lufs().unwrap() - before).abs() < 0.3,
            "silence does not pull the reading down ({} vs {before})",
            m2.lufs().unwrap()
        );
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
    fn planned_gain_keeps_the_peak_below_full_scale() {
        // Wants +5.3 dB, but the track peaks at 0.70 FS (-3.1 dB) and the EQ adds 3.4 dB.
        let g = plan_gain_db(5.3, 0.70, 3.4);
        let out_peak = 0.70 * 10f32.powf((g + 3.4) / 20.0);
        assert!(
            out_peak <= 10f32.powf(-HEADROOM_MARGIN_DB / 20.0) + 1e-4,
            "{out_peak}"
        );
        assert!(g < 5.3, "reduced from what the target asked for");
    }

    #[test]
    fn planned_gain_is_untouched_when_there_is_room() {
        assert_eq!(plan_gain_db(2.0, 0.3, 0.0), 2.0);
        assert_eq!(
            plan_gain_db(-4.0, 0.9, 3.0),
            -4.0,
            "attenuation is never held back"
        );
    }
}
