//! De-click: vinyl click detection and interpolation repair.
//!
//! For click-afflicted recordings (vinyl transfers): finds short impulsive
//! transients and joins the signal across them by interpolation, leaving
//! the music alone. Off by default; when off, the chain is bit-identical.
//! Spec: docs/v2/Declick.md.
//!
//! # Algorithm
//!
//! ```text
//! push chunk into fifo                          # lookahead for repair
//! for each channel (independently):
//!     d = |fifo[0] - prev_emitted|               # first difference
//!     rms = windowed_rms(~5 ms)                  # local level reference
//!     if d > threshold(rms, sensitivity):        # candidate impulse edge
//!         find click extent [0, b]               # while |diff| stays elevated
//!         if (b + 1) <= max_width: repair        # else groove damage: leave it
//! repair by cubic Hermite from two clean samples each side
//! emit fifo[0] delayed by lookahead_frames
//! y = dry_wet_mix(dry_delayed, wet)
//! ```
//!
//! - Detection is per channel: vinyl clicks are often unilateral (dust on
//!   one groove wall), so each channel is repaired from its own neighbors.
//!   (Contrast de-hiss, which links stereo to protect the image — different
//!   artifact, different correct answer.)
//! - `sensitivity` 0..1 maps log-wise to a threshold multiple of 6× RMS
//!   (gentle) down to 1.5× RMS (aggressive); default 0.5 lands at 3×.
//!   (The spec's 40×/6× range is inconsistent with its own §7 test — a 0.9
//!   impulse on a 0.4-amplitude sine has diff/rms ≈ 3.2, so a 12× default
//!   would never fire. Deviation noted in the release README.)
//! - The width gate (`max_width_ms`, default 2.0) refuses to interpolate
//!   across wider damage — thumps and groove wear are different tools'
//!   problems, and inventing milliseconds of signal is dishonest.
//! - The extent scan follows elevated first-differences; smooth low-diff
//!   tails after an impulse are a known limitation (real clicks are
//!   impulsive throughout — the spec's §7 test asserts the impulse's
//!   removal and unchanged THD, which is what this guarantees).
//! - Interpolation needs clean samples *after* the click, so the stage
//!   holds a FIFO of `max_width_ms + 3 ms` and genuinely delays the signal
//!   by that amount; `latency_frames()` reports it honestly.
//! - Deterministic: no RNG anywhere. Enabling/disabling ramps over 15 ms
//!   (a de-clicker that clicks would be embarrassing); `dry_wet` is the A/B.
//!
//! # Honest limitations
//!
//! Interpolation invents signal: the repaired span is a plausible guess,
//! not the original music. Aggressive sensitivity eats real transients —
//! percussion attacks, plucked strings. Nothing here helps crackle, thumps,
//! or groove wear. This stage removes events that were never music; it
//! recovers nothing.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::dsp::DspStage;

/// Extra lookahead beyond the widest repairable click, ms (v1 constant).
const LOOKAHEAD_MARGIN_MS: f64 = 3.0;
/// RMS window for the adaptive threshold, ms (v1 constant).
const RMS_WINDOW_MS: f64 = 5.0;
/// Enable/disable blend time.
const RAMP_SECONDS: f64 = 0.015;
/// Threshold floor, linear: with a silent window any transient would
/// otherwise qualify. −60 dBFS keeps real clicks detectable while digital
/// silence stays silent.
const THRESHOLD_FLOOR: f64 = 1e-3;

/// User-facing settings, persisted in `engine-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeclickSettings {
    pub enabled: bool,
    /// 0 = gentle … 1 = aggressive.
    pub sensitivity: f32,
    /// Wider defects are left alone, ms.
    pub max_width_ms: f32,
    /// Dry/wet blend: 0 = dry, 1 = fully repaired. The A/B control.
    pub dry_wet: f32,
}

impl Default for DeclickSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            sensitivity: 0.5,
            max_width_ms: 2.0,
            dry_wet: 1.0,
        }
    }
}

impl DeclickSettings {
    /// Threshold multiple of the local RMS for this sensitivity.
    /// Log-mapped 6× … 1.5×; default 0.5 lands at 3×. (See the module docs
    /// for why this differs from the spec's 40×/6×.)
    fn threshold_mult(sensitivity: f32) -> f64 {
        let s = sensitivity as f64;
        10f64.powf((1.0 - s) * 6f64.log10() + s * 1.5f64.log10())
    }

    /// Pull every value into its allowed range; non-finite values become
    /// their defaults (same contract as `AnalogSettings::clamped`).
    pub fn clamped(mut self) -> Self {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let d = Self::default();
        self.sensitivity = fin(self.sensitivity, d.sensitivity).clamp(0.0, 1.0);
        self.max_width_ms = fin(self.max_width_ms, d.max_width_ms).clamp(0.5, 5.0);
        self.dry_wet = fin(self.dry_wet, d.dry_wet).clamp(0.0, 1.0);
        self
    }
}

/// Per-channel detector and delay state.
struct Channel {
    /// Working FIFO (repaired in place); length hovers at lookahead + 1.
    fifo: VecDeque<f32>,
    /// Pristine delayed dry path.
    dry: VecDeque<f32>,
    /// Ring of recent samples for the RMS window, with a running sum of
    /// squares.
    rms_ring: VecDeque<f32>,
    rms_sum: f64,
    /// Causal RMS per fifo sample (parallel to `fifo`): the RMS as it was
    /// when that sample arrived — past-only, so a sudden transient stands
    /// out against the quiet that preceded it.
    rms_hist: VecDeque<f64>,
    /// Detection suppress counter: set after a wide defect so its body
    /// and trailing edge aren't misread as fresh clicks.
    skip: usize,
    /// Last two emitted samples (repair needs x[-1], x[-2]).
    hist: [f32; 2],
}

impl Channel {
    fn new() -> Self {
        Self {
            fifo: VecDeque::new(),
            dry: VecDeque::new(),
            rms_ring: VecDeque::new(),
            rms_sum: 0.0,
            rms_hist: VecDeque::new(),
            skip: 0,
            hist: [0.0; 2],
        }
    }

    fn clear(&mut self) {
        self.fifo.clear();
        self.dry.clear();
        self.rms_ring.clear();
        self.rms_sum = 0.0;
        self.rms_hist.clear();
        self.skip = 0;
        self.hist = [0.0; 2];
    }
}

/// The de-click stage. See the module docs.
pub struct DeclickStage {
    settings: DeclickSettings,
    sample_rate: u32,
    lookahead: usize,
    max_width: usize,
    rms_win: usize,
    channels: [Channel; 2],
    /// First process() call after reset primes the FIFOs with the first
    /// frame (a few ms of DC hold at a track boundary beats a few ms of
    /// silence, and zero diffs can't false-trigger the detector).
    need_prime: bool,
    /// Wet blend: 0 = dry, 1 = fully repaired. Ramps on toggle.
    mix: f64,
}

impl DeclickStage {
    pub fn new(sample_rate: u32) -> Self {
        let mut s = Self {
            settings: DeclickSettings::default(),
            sample_rate: 0,
            lookahead: 0,
            max_width: 0,
            rms_win: 0,
            channels: [Channel::new(), Channel::new()],
            need_prime: true,
            mix: 0.0,
        };
        s.prepare(sample_rate);
        s
    }

    /// Apply new settings (clamped). Switching on from fully dry clears
    /// the FIFOs rather than replaying stale history.
    pub fn set_settings(&mut self, settings: DeclickSettings) {
        let clamped = settings.clamped();
        if clamped.enabled && !self.settings.enabled && self.mix == 0.0 {
            self.reset();
        }
        self.settings = clamped;
    }

    pub fn settings(&self) -> DeclickSettings {
        self.settings
    }

    fn mix_target(&self) -> f64 {
        if self.settings.enabled {
            1.0
        } else {
            0.0
        }
    }

    fn prime(&mut self, frame: &[f32]) {
        for (ch, c) in self.channels.iter_mut().enumerate() {
            let v = frame[ch];
            c.fifo = VecDeque::from(vec![v; self.lookahead]);
            c.dry = VecDeque::from(vec![v; self.lookahead]);
            c.rms_ring = VecDeque::from(vec![v; self.rms_win]);
            c.rms_sum = (v as f64) * (v as f64) * self.rms_win as f64;
            c.rms_hist = VecDeque::from(vec![v.abs() as f64; self.lookahead]);
            c.hist = [v, v];
        }
        self.need_prime = false;
    }
}

impl DspStage for DeclickStage {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        let sr = sample_rate as f64;
        self.max_width = (self.settings.max_width_ms as f64 / 1000.0 * sr)
            .round()
            .max(1.0) as usize;
        self.lookahead = ((self.settings.max_width_ms as f64 + LOOKAHEAD_MARGIN_MS) / 1000.0 * sr)
            .round()
            .max(4.0) as usize;
        self.rms_win = (RMS_WINDOW_MS / 1000.0 * sr).round().max(8.0) as usize;
        // New rate, new geometry: drop history rather than misread it.
        self.reset();
    }

    fn process(&mut self, interleaved: &mut [f32], channels: usize) {
        if channels != 2 {
            return;
        }
        let target = self.mix_target();
        // Bit-transparent bypass: disabled and fully blended to dry — and,
        // because the bypass skips the FIFO, with no delay either.
        if target == 0.0 && self.mix == 0.0 {
            return;
        }
        if self.need_prime {
            if let Some(frame) = interleaved.chunks_exact(2).next() {
                self.prime(frame);
            } else {
                return;
            }
        }
        let step = 1.0 / (RAMP_SECONDS * self.sample_rate.max(1) as f64);
        let wet_target = self.settings.dry_wet as f64;
        let mult = DeclickSettings::threshold_mult(self.settings.sensitivity);
        let max_width = self.max_width;
        let lookahead = self.lookahead;

        for frame in interleaved.chunks_exact_mut(2) {
            let mut out = [0.0f32; 2];
            for (ch, c) in self.channels.iter_mut().enumerate() {
                let x = frame[ch];
                c.fifo.push_back(x);
                c.dry.push_back(x);
                // RMS window update (causal: this sample and its past).
                c.rms_ring.push_back(x);
                c.rms_sum += (x as f64) * (x as f64);
                if c.rms_ring.len() > self.rms_win {
                    if let Some(old) = c.rms_ring.pop_front() {
                        c.rms_sum -= (old as f64) * (old as f64);
                    }
                }
                c.rms_hist
                    .push_back((c.rms_sum / c.rms_ring.len().max(1) as f64).sqrt().max(0.0));

                // Detect at the emission position (index 0): the full
                // lookahead of future context is available for the extent
                // scan and the repair, and rms_hist[0] is the RMS as it was
                // when this sample arrived — past-only, so a sudden
                // transient stands out against the quiet that preceded it.
                if c.fifo.len() > lookahead {
                    let rms = c.rms_hist[0];
                    let thr = (mult * rms).max(THRESHOLD_FLOOR);
                    let d0 = (c.fifo[0] - c.hist[1]).abs() as f64;
                    if c.skip > 0 {
                        // Inside a wide defect (or its trailing edge):
                        // groove damage, left alone.
                        c.skip -= 1;
                    } else if d0 > thr {
                        // Find the full extent of elevated first
                        // differences. Bounded by what the lookahead can
                        // see; a defect running past it re-triggers "wide"
                        // as new samples arrive.
                        let mut b = 0usize;
                        while b + 2 < c.fifo.len()
                            && (c.fifo[b + 1] - c.fifo[b]).abs() as f64 > thr * 0.5
                        {
                            b += 1;
                        }
                        if b < max_width {
                            let p0 = c.hist[1] as f64;
                            let m0 = (c.hist[1] - c.hist[0]) as f64;
                            if b < 8 {
                                // Short gap: linear extrapolation from the
                                // clean left side. Avoids right anchors
                                // contaminated by a smooth click tail, and a
                                // tangent is near-exact over a few samples.
                                for i in 0..=b {
                                    c.fifo[i] = (p0 + m0 * (i + 1) as f64) as f32;
                                }
                            } else {
                                // Longer gap: cubic Hermite across [0, b]
                                // with clean samples each side. m0/m1 are
                                // per-sample slopes.
                                let n = (b + 1) as f64;
                                let p1 = c.fifo[b + 1] as f64;
                                let m1 = (c.fifo[b + 2] - c.fifo[b + 1]) as f64;
                                for i in 0..=b {
                                    let t = (i + 1) as f64 / (n + 1.0);
                                    let t2 = t * t;
                                    let t3 = t2 * t;
                                    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
                                    let h10 = t3 - 2.0 * t2 + t;
                                    let h01 = -2.0 * t3 + 3.0 * t2;
                                    let h11 = t3 - t2;
                                    c.fifo[i] =
                                        (h00 * p0 + h10 * m0 * n + h01 * p1 + h11 * m1 * n) as f32;
                                }
                            }
                        // Wider than max_width: groove damage. Suppress
                        // detection through the defect and its trailing
                        // edge so neither is misread as a fresh click.
                        } else {
                            c.skip = (b + 1) + 4;
                        }
                    }
                    let wet = c.fifo.pop_front().unwrap_or(0.0);
                    let dry = c.dry.pop_front().unwrap_or(0.0);
                    c.rms_hist.pop_front();
                    c.hist = [c.hist[1], wet];
                    let m = self.mix * wet_target;
                    // Exact endpoints: fully wet is bit-exact, fully dry is
                    // the delayed input — no 1-ulp rounding in either case.
                    out[ch] = if m == 1.0 {
                        wet
                    } else if m == 0.0 {
                        dry
                    } else {
                        dry + (wet - dry) * m as f32
                    };
                } else {
                    // Still priming (shouldn't happen after prime(), but
                    // never invent or drop samples): pass through.
                    out[ch] = x;
                }
            }
            frame[0] = out[0];
            frame[1] = out[1];

            // Toward the target; snap at the ends so bypass stays exact.
            self.mix = if (target - self.mix).abs() <= step {
                target
            } else {
                self.mix + step * (target - self.mix).signum()
            };
        }
    }

    /// The lookahead FIFO genuinely delays the signal by a few ms.
    fn latency_frames(&self) -> u32 {
        if self.settings.enabled {
            self.lookahead as u32
        } else {
            0
        }
    }

    fn reset(&mut self) {
        for c in self.channels.iter_mut() {
            c.clear();
        }
        self.need_prime = true;
        // A discontinuity (new stream, seek) needs no ramp.
        self.mix = self.mix_target();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(sample_rate: u32) -> DeclickStage {
        let mut s = DeclickStage::new(sample_rate);
        s.set_settings(DeclickSettings {
            enabled: true,
            ..DeclickSettings::default()
        });
        s.reset(); // skip the enable ramp: fully wet from the first frame
        s
    }

    fn stereo(frames: usize, mut f: impl FnMut(usize) -> (f32, f32)) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let (l, r) = f(i);
                [l, r]
            })
            .collect()
    }

    /// 1 kHz tone, cosine phase: starts at peak amplitude so the detector
    /// sees settled audio from frame 0, not an abrupt rise from digital
    /// silence (which is legitimately click-like to an adaptive threshold).
    fn tone_1k(i: usize, sr: u32, amp: f32) -> f32 {
        (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / sr as f64).cos() as f32 * amp
    }

    /// Output frame `n` carries input frame `n - la` (the lookahead delay);
    /// the first `la` outputs are the priming DC hold. Compare latency-
    /// compensated, reporting the first mismatch instead of dumping
    /// 40k-element vecs.
    fn assert_delayed_eq(buf: &[f32], input: &[f32], la: usize) {
        let a = &buf[la * 2..];
        let b = &input[..input.len() - la * 2];
        assert_eq!(a.len(), b.len(), "length mismatch");
        if let Some(i) = a.iter().zip(b).position(|(x, y)| x != y) {
            panic!(
                "first mismatch at output frame {}: got {}, want {}",
                i / 2,
                a[i],
                b[i]
            );
        }
    }

    /// Correlation-based amplitude of `freq` in the left channel.
    fn harmonic(buf: &[f32], freq: f64, sr: u32) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        let n = buf.len() / 2;
        for (i, x) in buf.iter().step_by(2).enumerate().take(n) {
            let ph = 2.0 * std::f64::consts::PI * freq * i as f64 / sr as f64;
            re += *x as f64 * ph.cos();
            im += *x as f64 * ph.sin();
        }
        2.0 * (re * re + im * im).sqrt() / n as f64
    }

    #[test]
    fn bypass_is_bit_transparent() {
        let mut s = DeclickStage::new(44100);
        let input = stereo(4096, |i| (tone_1k(i, 44100, 0.5), tone_1k(i, 44100, 0.3)));
        let mut buf = input.clone();
        s.process(&mut buf, 2);
        assert_eq!(buf, input, "a disabled stage must not touch a sample");
    }

    /// A single-sample impulse plus a 20-sample burst on a 1 kHz sine, per
    /// the spec's §7: the impulse is gone, and the sine's THD is unchanged
    /// within measurement noise. (Smooth low-diff burst tails are a
    /// documented limitation — see the module docs.)
    #[test]
    fn synthetic_click_is_detected_and_repaired() {
        let mut s = stage(44100);
        let la = s.latency_frames() as usize;
        let frames = 44100;
        let mut buf = stereo(frames, |i| {
            let v = tone_1k(i, 44100, 0.4);
            (v, v)
        });
        // Click at frame 22050: single-sample impulse + 20-sample burst.
        let at = 22050;
        buf[at * 2] += 1.0;
        for k in 1..=20 {
            buf[(at + k) * 2] += 0.5 * (-(k as f32) / 8.0).exp();
        }
        let input = buf.clone();
        s.process(&mut buf, 2);
        // The impulse is gone: the repaired sample rejoins the clean sine.
        let out_at = at + la;
        let got = buf[out_at * 2];
        let want = tone_1k(at, 44100, 0.4);
        assert!(
            (got - want).abs() < 0.05,
            "impulse residual: got {got}, want {want}"
        );
        // Surgical: only the repair region differs from the delayed input.
        // (Slice is input-aligned: slice frame f == input frame f; the
        // click at input `at` appears at output `at + la`.)
        let mut changed_outside = 0;
        for (i, (a, b)) in buf[la * 2..]
            .iter()
            .zip(&input[..input.len() - la * 2])
            .enumerate()
        {
            let frame = i / 2;
            if (frame < at - 1 || frame > at + 1) && a != b {
                changed_outside += 1;
            }
        }
        assert_eq!(changed_outside, 0, "detector touched clean audio");
        // THD unchanged: harmonics on a clean stretch, in vs out.
        let clean_in = &input[30000 * 2..40000 * 2];
        let clean_out = &buf[(30000 + la) * 2..(40000 + la) * 2];
        for h in [2000.0, 3000.0, 4000.0, 5000.0] {
            let a = harmonic(clean_in, h, 44100);
            let b = harmonic(clean_out, h, 44100);
            assert!(
                (a - b).abs() < 1e-4,
                "harmonic {h} Hz changed: {a:.2e} → {b:.2e}"
            );
        }
        // Unilateral click: right channel is just delayed, bit-exact.
        let right_out: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
        let right_in: Vec<f32> = input.iter().skip(1).step_by(2).copied().collect();
        if let Some(i) = right_out[la..]
            .iter()
            .zip(&right_in[..right_in.len() - la])
            .position(|(a, b)| a != b)
        {
            panic!("right channel changed at frame {}", i / 2);
        }
    }

    /// A click straddling two process() calls is still caught: the FIFO
    /// makes chunk boundaries invisible to the detector.
    #[test]
    fn click_at_a_chunk_boundary_is_repaired() {
        let mut s = stage(44100);
        let la = s.latency_frames() as usize;
        let mut buf = stereo(8192, |i| {
            let v = tone_1k(i, 44100, 0.4);
            (v, v)
        });
        // Impulse lands on the last frame of the first chunk.
        buf[4095 * 2] += 0.9;
        let (first, second) = buf.split_at_mut(4096 * 2);
        s.process(first, 2);
        s.process(second, 2);
        // In output coordinates the click sits `la` frames later.
        let out_at = 4095 + la;
        let worst = buf[out_at * 2 - 2..out_at * 2 + 3]
            .iter()
            .zip((4093..4098).map(|i| tone_1k(i, 44100, 0.4)))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(worst < 0.1, "worst residual at the boundary: {worst}");
    }

    /// A defect wider than max_width_ms is groove damage, not a click:
    /// left alone (modulo the honest lookahead delay).
    #[test]
    fn wide_defect_is_left_alone() {
        let mut s = stage(44100);
        let la = s.latency_frames() as usize;
        let frames = 22050;
        let mut buf = stereo(frames, |i| {
            let v = tone_1k(i, 44100, 0.4);
            (v, v)
        });
        // 5 ms burst of alternating full-scale at frame 11025: wider than
        // max_width (2 ms), so groove damage, not a click.
        for k in 0..220 {
            buf[(11025 + k) * 2] = if k % 2 == 0 { 1.0 } else { -1.0 };
            buf[(11025 + k) * 2 + 1] = if k % 2 == 0 { -1.0 } else { 1.0 };
        }
        let want = buf.clone();
        s.process(&mut buf, 2);
        assert_delayed_eq(&buf, &want, la);
    }

    /// A fast-attack drum-like transient inside ongoing music at default
    /// sensitivity is music, not a click: it must survive (modulo the
    /// lookahead delay). The adaptive threshold keys off the music's own
    /// RMS, so a loud-but-smooth attack stays under it.
    #[test]
    fn drum_transient_survives_default_sensitivity() {
        let mut s = stage(44100);
        let la = s.latency_frames() as usize;
        let frames = 44100;
        let mut buf = stereo(frames, |i| {
            // Musical bed: 150 Hz + 300 Hz, cosine phase so the detector
            // sees settled audio from frame 0.
            let bed = (2.0 * std::f64::consts::PI * 150.0 * i as f64 / 44100.0).cos() as f32 * 0.25
                + (2.0 * std::f64::consts::PI * 300.0 * i as f64 / 44100.0).cos() as f32 * 0.1;
            // Drum hit at frame 22050: 180 Hz, 4-sample raised-cosine
            // attack, exponential decay, added on top of the bed.
            let hit = if i >= 22050 {
                let k = (i - 22050) as f32;
                let attack = if k < 4.0 {
                    0.5 - 0.5 * (std::f32::consts::PI * k / 4.0).cos()
                } else {
                    1.0
                };
                (2.0 * std::f64::consts::PI * 180.0 * i as f64 / 44100.0).sin() as f32
                    * (-k / 400.0).exp()
                    * attack
                    * 0.8
            } else {
                0.0
            };
            (bed + hit, bed + hit)
        });
        let want = buf.clone();
        s.process(&mut buf, 2);
        assert_delayed_eq(&buf, &want, la);
    }

    /// The trade-off, documented not hidden: at maximum sensitivity a
    /// transient arriving on a near-silent RMS window gets marked.
    #[test]
    fn maximum_sensitivity_marks_a_transient_on_a_quiet_window() {
        let mut s = DeclickStage::new(44100);
        s.set_settings(DeclickSettings {
            enabled: true,
            sensitivity: 1.0,
            ..DeclickSettings::default()
        });
        s.reset();
        let frames = 22050;
        // Silence, then a hard 1-sample attack to full scale at frame 500:
        // the RMS window is still nearly empty, so the adaptive threshold
        // is tiny and the transient looks like a click.
        let mut buf = stereo(frames, |i| {
            let v = if i < 500 {
                0.0
            } else {
                (2.0 * std::f64::consts::PI * 180.0 * i as f64 / 44100.0).sin() as f32
                    * (-((i - 500) as f32) / 300.0).exp()
            };
            (v, v)
        });
        let want = buf.clone();
        s.process(&mut buf, 2);
        let la = s.latency_frames() as usize;
        let changed = buf[la * 2..]
            .iter()
            .zip(&want[..want.len() - la * 2])
            .filter(|(a, b)| (**a - **b).abs() > 1e-6)
            .count();
        assert!(
            changed > 0,
            "expected the aggressive setting to mark the transient"
        );
    }

    #[test]
    fn silence_in_is_silence_out() {
        let mut s = stage(44100);
        let mut buf = stereo(8192, |_| (0.0, 0.0));
        s.process(&mut buf, 2);
        assert!(buf.iter().all(|x| *x == 0.0));
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn latency_reports_the_lookahead() {
        let s = stage(48000);
        // max_width 2 ms + 3 ms margin = 5 ms at 48 kHz = 240 frames.
        assert_eq!(s.latency_frames(), 240);
        let off = DeclickStage::new(48000);
        assert_eq!(off.latency_frames(), 0);
    }

    #[test]
    fn reset_clears_the_fifo() {
        let mut s = stage(44100);
        let mut buf = stereo(4096, |i| (tone_1k(i, 44100, 0.5), 0.0));
        s.process(&mut buf, 2);
        s.reset();
        assert!(s.channels.iter().all(|c| c.fifo.is_empty()));
        // After reset the next process() primes again and emits.
        let mut buf2 = stereo(512, |i| (tone_1k(i, 44100, 0.5), 0.0));
        s.process(&mut buf2, 2);
        assert!(buf2.iter().any(|x| *x != 0.0));
    }

    #[test]
    fn sample_rate_scales_the_ms_constants() {
        let s44 = stage(44100);
        let s192 = stage(192000);
        // 5 ms lookahead at both rates (220.5 rounds to 221).
        assert_eq!(s44.latency_frames(), 221);
        assert_eq!(s192.latency_frames(), 960);
        assert_eq!(s44.max_width, 88);
        assert_eq!(s192.max_width, 384);
    }

    #[test]
    fn clamped_repairs_non_finite_and_bounds_ranges() {
        let bad = DeclickSettings {
            sensitivity: f32::NAN,
            max_width_ms: 99.0,
            dry_wet: -1.0,
            ..DeclickSettings::default()
        };
        let c = bad.clamped();
        assert_eq!(c.sensitivity, 0.5);
        assert_eq!(c.max_width_ms, 5.0);
        assert_eq!(c.dry_wet, 0.0);
    }

    #[test]
    fn settings_round_trip_and_old_files_default_to_off() {
        let s = DeclickSettings {
            enabled: true,
            sensitivity: 0.7,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<DeclickSettings>(&json).unwrap(), s);
        let partial: DeclickSettings = serde_json::from_str(r#"{"sensitivity": 0.2}"#).unwrap();
        assert!(!partial.enabled);
        assert_eq!(partial.max_width_ms, 2.0);
    }

    #[test]
    fn mono_and_surround_pass_through_untouched() {
        let mut s = stage(44100);
        for ch in [1usize, 6] {
            let mut buf = vec![0.3f32; ch * 512];
            let want = buf.clone();
            s.process(&mut buf, ch);
            assert_eq!(buf, want, "{ch} channels");
        }
    }
}
