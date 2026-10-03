//! Headphone crossfeed: a stereo-only PCM DSP stage.
//!
//! Crossfeed bleeds a filtered copy of each channel into the other, restoring
//! some of the interaural level and timing cues that loudspeakers provide and
//! headphones remove. It is a subtle spatial effect, not a room simulator.
//! Spec: docs/v1/kahawai-crossfeed-dsp-spec.md.
//!
//! # Algorithm
//!
//! A clean-room reimplementation of Boris Mikhaylov's **bs2b** signal path,
//! checked against the libbs2b 3.1.0 reference (`init()` / `cross_feed_d()`).
//! Per stereo frame:
//!
//! ```text
//! lo_L = a0_lo * L + b1_lo * lo_L_prev                     (one-pole low-pass)
//! hi_L = a0_hi * L + a1_hi * L_prev + b1_hi * hi_L_prev    (high shelf, direct path)
//! L'   = (hi_L + lo_R) * gain                              (plus the other side's low-pass)
//! ```
//!
//! Coefficients come from the preset's `(feed_db, cutoff_hz)` exactly as in
//! the reference:
//!
//! ```text
//! GB_lo = feed * -5/6 - 3          GB_hi = feed / 6 - 3
//! G_lo  = 10^(GB_lo/20)            G_hi  = 1 - 10^(GB_hi/20)
//! Fc_hi = Fc_lo * 2^((GB_lo - 20*log10(G_hi)) / 12)
//! x     = exp(-2*pi*Fc / sample_rate)
//! b1_lo = x            a0_lo = G_lo * (1 - x)
//! b1_hi = x            a0_hi = 1 - G_hi * (1 - x)     a1_hi = -x
//! gain  = 1 / (1 - G_hi + G_lo)
//! ```
//!
//! - Coefficients and filter state are `f64`, as in the reference; only the
//!   edges are `f32`.
//! - There is no delay line: the timing cue comes from the filters' phase, so
//!   `latency_frames()` is 0. (The spec assumed a sub-millisecond delay line;
//!   the reference has none.)
//! - Like `bs2b_set_level`, a preset change keeps the filter state. Unlike
//!   the reference, enabling and disabling ramp the wet/dry blend over 15 ms,
//!   so toggling never clicks.
//!
//! No Linkwitz preset yet: its 1971 circuit values have not been verified,
//! and a preset under that name must be digitized from the real circuit.
//!
//! # Placement
//!
//! First in the PCM chain: `crossfeed -> EQ -> analog -> loudness gain ->
//! volume -> look-ahead limiter -> headroom guard`. It models the acoustics
//! of speakers, so the EQ (which corrects the headphones) shapes the signal
//! as it will reach the ear. Stereo only; any other layout passes through.
//! Exclusive output (DoP or bit-perfect PCM) never calls it, and while it is
//! enabled Auto does not choose exclusive output (see `exclusive_blockers`).

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::dsp::DspStage;

/// Enable/disable blend time (the EQ's ramp uses the same 15 ms).
const RAMP_SECONDS: f64 = 0.015;

/// The classic voicings, from the bs2b level table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossfeedPreset {
    /// 700 Hz / 4.5 dB: bs2b's default, "closest to virtual speakers", and
    /// the strongest of the three. (bs2b's "feed" is not the crossfed level:
    /// a higher figure crossfeeds less. Measured on real tracks, Bauer cuts
    /// the side signal by about 5.5 dB, Chu Moy 5, Meier 3.5.)
    Bauer,
    /// 700 Hz / 6.0 dB.
    ChuMoy,
    /// 650 Hz / 9.5 dB: the mildest of the three.
    Meier,
    /// The settings' own cutoff and feed.
    Custom,
}

impl CrossfeedPreset {
    /// `(cutoff_hz, feed_db)` of a named preset; `None` for `Custom`.
    pub fn params(self) -> Option<(f32, f32)> {
        match self {
            CrossfeedPreset::Bauer => Some((700.0, 4.5)),
            CrossfeedPreset::ChuMoy => Some((700.0, 6.0)),
            CrossfeedPreset::Meier => Some((650.0, 9.5)),
            CrossfeedPreset::Custom => None,
        }
    }
}

/// User-facing settings, persisted in `engine-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrossfeedSettings {
    pub enabled: bool,
    pub preset: CrossfeedPreset,
    /// Low-pass cutoff of the crossfed path, Hz. Used when `preset` is Custom.
    pub cutoff_hz: f32,
    /// Level of the crossfed path, dB. Used when `preset` is Custom.
    pub feed_db: f32,
}

impl Default for CrossfeedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: CrossfeedPreset::Bauer,
            cutoff_hz: 700.0,
            feed_db: 4.5,
        }
    }
}

impl CrossfeedSettings {
    /// Custom cutoff range, Hz.
    pub const CUTOFF_RANGE: (f32, f32) = (200.0, 2000.0);
    /// Custom feed range, dB.
    pub const FEED_RANGE: (f32, f32) = (0.5, 15.0);

    /// Ranges pulled into bounds; `None` if a value is not finite.
    pub fn clamped(&self) -> Option<Self> {
        if !self.cutoff_hz.is_finite() || !self.feed_db.is_finite() {
            return None;
        }
        let (cut_lo, cut_hi) = Self::CUTOFF_RANGE;
        let (feed_lo, feed_hi) = Self::FEED_RANGE;
        Some(Self {
            cutoff_hz: self.cutoff_hz.clamp(cut_lo, cut_hi),
            feed_db: self.feed_db.clamp(feed_lo, feed_hi),
            ..*self
        })
    }

    /// `(cutoff_hz, feed_db)` in effect: the preset's, or the settings' own.
    fn effective_params(&self) -> (f64, f64) {
        match self.preset.params() {
            Some((cut, feed)) => (cut as f64, feed as f64),
            None => (self.cutoff_hz as f64, self.feed_db as f64),
        }
    }
}

/// bs2b recurrence coefficients.
#[derive(Debug, Clone, Copy)]
struct Coeffs {
    a0_lo: f64,
    b1_lo: f64,
    a0_hi: f64,
    a1_hi: f64,
    b1_hi: f64,
    gain: f64,
}

/// libbs2b 3.1.0 `init()`.
fn design_bs2b(feed_db: f64, cutoff_hz: f64, sample_rate: f64) -> Coeffs {
    let gb_lo = feed_db * -5.0 / 6.0 - 3.0;
    let gb_hi = feed_db / 6.0 - 3.0;
    let g_lo = 10f64.powf(gb_lo / 20.0);
    let g_hi = 1.0 - 10f64.powf(gb_hi / 20.0);
    let fc_hi = cutoff_hz * 2f64.powf((gb_lo - 20.0 * g_hi.log10()) / 12.0);

    let x = (-2.0 * PI * cutoff_hz / sample_rate).exp();
    let (a0_lo, b1_lo) = (g_lo * (1.0 - x), x);
    let x = (-2.0 * PI * fc_hi / sample_rate).exp();
    let (a0_hi, a1_hi, b1_hi) = (1.0 - g_hi * (1.0 - x), -x, x);

    Coeffs {
        a0_lo,
        b1_lo,
        a0_hi,
        a1_hi,
        b1_hi,
        gain: 1.0 / (1.0 - g_hi + g_lo),
    }
}

/// The crossfeed stage. See the module docs.
pub struct CrossfeedStage {
    settings: CrossfeedSettings,
    coeffs: Coeffs,
    sample_rate: u32,
    /// Wet/dry blend: 0 = dry, 1 = fully crossfed. Ramps on toggle.
    mix: f64,
    /// Filter state per channel [left, right].
    lo: [f64; 2],
    hi: [f64; 2],
    prev_in: [f64; 2],
}

impl CrossfeedStage {
    pub fn new(sample_rate: u32) -> Self {
        let settings = CrossfeedSettings::default();
        let (cutoff, feed) = settings.effective_params();
        Self {
            settings,
            coeffs: design_bs2b(feed, cutoff, sample_rate as f64),
            sample_rate,
            mix: 0.0,
            lo: [0.0; 2],
            hi: [0.0; 2],
            prev_in: [0.0; 2],
        }
    }

    /// Apply new settings (clamped). Non-finite values are ignored and the
    /// previous settings kept.
    pub fn set_settings(&mut self, settings: CrossfeedSettings) {
        let Some(clamped) = settings.clamped() else {
            return;
        };
        // Switched on from fully dry: the filters last ran long ago (or
        // never), so start them clean rather than replay stale history.
        if clamped.enabled && !self.settings.enabled && self.mix == 0.0 {
            self.clear_state();
        }
        self.settings = clamped;
        let (cutoff, feed) = clamped.effective_params();
        self.coeffs = design_bs2b(feed, cutoff, self.sample_rate as f64);
    }

    pub fn settings(&self) -> CrossfeedSettings {
        self.settings
    }

    fn clear_state(&mut self) {
        self.lo = [0.0; 2];
        self.hi = [0.0; 2];
        self.prev_in = [0.0; 2];
    }

    fn mix_target(&self) -> f64 {
        if self.settings.enabled {
            1.0
        } else {
            0.0
        }
    }
}

impl DspStage for CrossfeedStage {
    fn prepare(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            let (cutoff, feed) = self.settings.effective_params();
            self.coeffs = design_bs2b(feed, cutoff, sample_rate as f64);
            // History from another rate would ring.
            self.clear_state();
        }
    }

    fn process(&mut self, interleaved: &mut [f32], channels: usize) {
        if channels != 2 {
            return;
        }
        let target = self.mix_target();
        // Bit-transparent bypass: disabled and fully blended to dry.
        if target == 0.0 && self.mix == 0.0 {
            return;
        }
        let c = self.coeffs;
        let step = 1.0 / (RAMP_SECONDS * self.sample_rate.max(1) as f64);

        for frame in interleaved.chunks_exact_mut(2) {
            let (l, r) = (frame[0] as f64, frame[1] as f64);

            // libbs2b cross_feed_d().
            let lo_l = c.a0_lo * l + c.b1_lo * self.lo[0];
            let lo_r = c.a0_lo * r + c.b1_lo * self.lo[1];
            let hi_l = c.a0_hi * l + c.a1_hi * self.prev_in[0] + c.b1_hi * self.hi[0];
            let hi_r = c.a0_hi * r + c.a1_hi * self.prev_in[1] + c.b1_hi * self.hi[1];
            self.prev_in = [l, r];
            self.lo = [lo_l, lo_r];
            self.hi = [hi_l, hi_r];
            let wet_l = (hi_l + lo_r) * c.gain;
            let wet_r = (hi_r + lo_l) * c.gain;

            let m = self.mix;
            frame[0] = (l + (wet_l - l) * m) as f32;
            frame[1] = (r + (wet_r - r) * m) as f32;

            // Toward the target; snap at the ends so bypass stays exact.
            self.mix = if (target - m).abs() <= step {
                target
            } else {
                m + step * (target - m).signum()
            };
        }
    }

    /// No delay line, so no latency.
    fn latency_frames(&self) -> u32 {
        0
    }

    fn reset(&mut self) {
        self.clear_state();
        // A discontinuity (new stream, seek) needs no ramp.
        self.mix = self.mix_target();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(preset: CrossfeedPreset, sample_rate: u32) -> CrossfeedStage {
        let mut s = CrossfeedStage::new(sample_rate);
        s.set_settings(CrossfeedSettings {
            enabled: true,
            preset,
            ..CrossfeedSettings::default()
        });
        s.reset(); // skip the enable ramp: fully crossfed from the first frame
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

    /// Deterministic noise in [-0.5, 0.5).
    fn noise(seed: u32) -> impl FnMut() -> f32 {
        let mut x = seed.max(1);
        move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32) - 0.5
        }
    }

    /// A direct, independent transcription of libbs2b's `init()` +
    /// `cross_feed_d()` for one buffer, starting from zero state.
    fn reference_bs2b(input: &[f32], feed_db: f64, cutoff_hz: f64, sr: f64) -> Vec<f32> {
        let gb_lo = feed_db * -5.0 / 6.0 - 3.0;
        let gb_hi = feed_db / 6.0 - 3.0;
        let g_lo = 10f64.powf(gb_lo / 20.0);
        let g_hi = 1.0 - 10f64.powf(gb_hi / 20.0);
        let fc_hi = cutoff_hz * 2f64.powf((gb_lo - 20.0 * g_hi.log10()) / 12.0);
        let x_lo = (-2.0 * PI * cutoff_hz / sr).exp();
        let x_hi = (-2.0 * PI * fc_hi / sr).exp();
        let gain = 1.0 / (1.0 - g_hi + g_lo);
        let (mut lo, mut hi, mut asis) = ([0.0f64; 2], [0.0f64; 2], [0.0f64; 2]);
        let mut out = Vec::with_capacity(input.len());
        for fr in input.chunks_exact(2) {
            let s = [fr[0] as f64, fr[1] as f64];
            for c in 0..2 {
                lo[c] = g_lo * (1.0 - x_lo) * s[c] + x_lo * lo[c];
                hi[c] = (1.0 - g_hi * (1.0 - x_hi)) * s[c] - x_hi * asis[c] + x_hi * hi[c];
                asis[c] = s[c];
            }
            out.push(((hi[0] + lo[1]) * gain) as f32);
            out.push(((hi[1] + lo[0]) * gain) as f32);
        }
        out
    }

    /// Gain of the crossfed path (left in, right out) at `freq`, from a
    /// settled sine: RMS out over RMS in.
    fn crossfed_gain(preset: CrossfeedPreset, sr: u32, freq: f64) -> f64 {
        let mut s = stage(preset, sr);
        let frames = sr as usize; // 1 s: long enough to settle
        let mut buf = stereo(frames, |i| {
            ((2.0 * PI * freq * i as f64 / sr as f64).sin() as f32, 0.0)
        });
        s.process(&mut buf, 2);
        let tail = &buf[buf.len() / 2..];
        let rms = |it: &mut dyn Iterator<Item = f32>| {
            let v: Vec<f64> = it.map(|x| x as f64).collect();
            (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
        };
        rms(&mut tail.iter().skip(1).step_by(2).copied()) / (0.5f64).sqrt()
    }

    #[test]
    fn bypass_is_bit_transparent() {
        let mut s = CrossfeedStage::new(44100);
        let mut n = noise(7);
        let input = stereo(4096, |_| (n(), n()));
        let mut buf = input.clone();
        s.process(&mut buf, 2);
        assert_eq!(buf, input, "a disabled stage must not touch a sample");
    }

    #[test]
    fn named_presets_are_the_bs2b_values() {
        assert_eq!(CrossfeedPreset::Bauer.params(), Some((700.0, 4.5)));
        assert_eq!(CrossfeedPreset::ChuMoy.params(), Some((700.0, 6.0)));
        assert_eq!(CrossfeedPreset::Meier.params(), Some((650.0, 9.5)));
        assert_eq!(CrossfeedPreset::Custom.params(), None);
    }

    /// Sample by sample against an independent transcription of the
    /// reference, on noise, at every common rate.
    #[test]
    fn matches_the_bs2b_reference_sample_by_sample() {
        for preset in [
            CrossfeedPreset::Bauer,
            CrossfeedPreset::ChuMoy,
            CrossfeedPreset::Meier,
        ] {
            let (cut, feed) = preset.params().unwrap();
            for sr in [44_100u32, 48_000, 96_000, 192_000] {
                let mut n = noise(sr);
                let input = stereo(4096, |_| (n(), n()));
                let mut buf = input.clone();
                stage(preset, sr).process(&mut buf, 2);
                let want = reference_bs2b(&input, feed as f64, cut as f64, sr as f64);
                let worst = buf
                    .iter()
                    .zip(&want)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f32::max);
                assert!(worst < 1e-6, "{preset:?} @ {sr} Hz: max difference {worst}");
            }
        }
    }

    /// The design scales with the rate, so what you hear doesn't: the
    /// crossfed path's response is the same at 44.1 and 192 kHz.
    #[test]
    fn the_response_does_not_depend_on_the_sample_rate() {
        for freq in [100.0, 700.0, 3000.0] {
            let a = crossfed_gain(CrossfeedPreset::Bauer, 44_100, freq);
            let b = crossfed_gain(CrossfeedPreset::Bauer, 192_000, freq);
            let diff_db = 20.0 * (a / b).log10();
            assert!(diff_db.abs() < 0.2, "{freq} Hz: {diff_db:.3} dB apart");
        }
        // And it is a low-pass: far less crossfeed at 3 kHz than at 100 Hz.
        let low = crossfed_gain(CrossfeedPreset::Bauer, 48_000, 100.0);
        let high = crossfed_gain(CrossfeedPreset::Bauer, 48_000, 3000.0);
        assert!(high < low / 2.0, "100 Hz {low:.3}, 3 kHz {high:.3}");
    }

    #[test]
    fn a_hard_panned_signal_reaches_the_other_ear_at_a_lower_level() {
        let mut s = stage(CrossfeedPreset::Bauer, 44_100);
        let mut buf = stereo(88_200, |_| (1.0, 0.0));
        s.process(&mut buf, 2);
        let (l, r) = (buf[buf.len() - 2], buf[buf.len() - 1]);
        assert!(l > 0.5 && r > 0.01 && r < l, "left {l}, right {r}");
    }

    #[test]
    fn mono_and_surround_pass_through_untouched() {
        let mut s = stage(CrossfeedPreset::Meier, 44_100);
        for ch in [1usize, 6] {
            let mut buf = vec![0.3f32; ch * 512];
            let want = buf.clone();
            s.process(&mut buf, ch);
            assert_eq!(buf, want, "{ch} channels");
        }
    }

    #[test]
    fn enabling_ramps_in_instead_of_jumping() {
        let mut s = CrossfeedStage::new(48_000);
        s.set_settings(CrossfeedSettings {
            enabled: true,
            ..CrossfeedSettings::default()
        });
        let mut buf = stereo(4800, |_| (1.0, 0.0)); // 100 ms, hard left
        s.process(&mut buf, 2);
        // Right channel goes from dry (0) toward crossfed, without a step.
        let right: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(right[0], 0.0);
        let max_step = right
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(max_step < 0.01, "largest sample-to-sample jump {max_step}");
        assert_eq!(s.mix, 1.0, "fully crossfed after the 15 ms ramp");
    }

    #[test]
    fn disabling_returns_to_an_exact_bypass() {
        let mut s = stage(CrossfeedPreset::Bauer, 48_000);
        s.set_settings(CrossfeedSettings::default()); // enabled: false
        let mut ramp = stereo(4800, |_| (0.5, -0.5));
        s.process(&mut ramp, 2);
        let mut n = noise(3);
        let input = stereo(1024, |_| (n(), n()));
        let mut buf = input.clone();
        s.process(&mut buf, 2);
        assert_eq!(buf, input);
    }

    #[test]
    fn latency_is_zero() {
        assert_eq!(stage(CrossfeedPreset::Bauer, 48_000).latency_frames(), 0);
    }

    #[test]
    fn silence_and_tiny_values_stay_finite() {
        let mut s = stage(CrossfeedPreset::Meier, 192_000);
        let mut buf = stereo(
            192_000,
            |i| if i < 10 { (1e-38, -1e-38) } else { (0.0, 0.0) },
        );
        s.process(&mut buf, 2);
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn clamped_bounds_custom_values_and_rejects_non_finite() {
        let bad = CrossfeedSettings {
            cutoff_hz: f32::NAN,
            ..CrossfeedSettings::default()
        };
        assert!(bad.clamped().is_none());
        let wild = CrossfeedSettings {
            cutoff_hz: 20_000.0,
            feed_db: -5.0,
            ..CrossfeedSettings::default()
        };
        let c = wild.clamped().unwrap();
        assert_eq!((c.cutoff_hz, c.feed_db), (2000.0, 0.5));

        let mut s = CrossfeedStage::new(44_100);
        let before = s.settings();
        s.set_settings(bad);
        assert_eq!(
            s.settings(),
            before,
            "non-finite input never reaches the stage"
        );
    }

    #[test]
    fn custom_uses_its_own_cutoff_and_feed() {
        let s = CrossfeedSettings {
            preset: CrossfeedPreset::Custom,
            cutoff_hz: 1000.0,
            feed_db: 8.0,
            ..CrossfeedSettings::default()
        };
        assert_eq!(s.effective_params(), (1000.0, 8.0));
    }

    #[test]
    fn settings_round_trip_and_old_files_default_to_off() {
        let s = CrossfeedSettings {
            enabled: true,
            preset: CrossfeedPreset::ChuMoy,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"chu_moy\""));
        assert_eq!(serde_json::from_str::<CrossfeedSettings>(&json).unwrap(), s);
        let partial: CrossfeedSettings = serde_json::from_str(r#"{"preset":"meier"}"#).unwrap();
        assert!(!partial.enabled);
        assert_eq!(partial.preset, CrossfeedPreset::Meier);
    }
}
