//! De-hiss: a multiband downward expander for steady background hiss.
//!
//! For noisy recordings (tape transfers, noisy needledrops): splits the
//! signal into four bands and turns each band down when its level falls
//! below a threshold, so hiss drops in the gaps and quiet passages while
//! the music itself passes untouched. It does nothing while the signal
//! masks the noise — the most musical of the classical approaches, and the
//! one that fits the `DspStage` contract (in-place, no block latency, no
//! new dependencies). Spec: docs/v2/Dehiss.md.
//!
//! # Algorithm
//!
//! ```text
//! bands = lr4_split(x, [400, 2500, 8000])   # 4 bands, Linkwitz-Riley 4th order
//! for each band b:
//!     env  = follower(max(|L_b|, |R_b|), attack, release)  # stereo-linked
//!     g    = expander_gain(env, threshold_b, ratio, knee, amount)  # <= 1
//!     y_b  = bands[b] * g
//! y = sum(y_b)                              # LR4 sums flat
//! y = dry_wet_mix(x, y)
//! ```
//!
//! - Crossover: Linkwitz–Riley 4th order per split — two cascaded
//!   second-order Butterworth low-passes, with the high-pass formed by
//!   subtraction (`x - lowpassed`). LR4 halves are in phase, so the four
//!   bands sum back to the input (up to float rounding).
//! - Gain computer: 2:1 ratio below the threshold, 6 dB soft knee, maximum
//!   reduction `amount_db`. These are v1 constants, documented here — not
//!   controls. Gain never exceeds 1.0: this stage only turns things down.
//! - Stereo linking: the envelope per band is `max(L, R)` and the gain
//!   applies to both channels, so the image never wanders when one channel
//!   dips below threshold alone.
//! - Enabling/disabling ramps the wet blend over 15 ms, so toggling never
//!   clicks. `dry_wet` is the A/B control.
//!
//! # Honest limitations
//!
//! Single-ended expansion cannot tell hiss from quiet high-frequency
//! signal — brushed drums, room tone, reverb tails, fade-outs get ducked
//! with the noise. It does nothing for clicks, pops, hum, or non-stationary
//! noise. Mis-set it breathes and pumps; the defaults are conservative.

use serde::{Deserialize, Serialize};

use super::biquad::{biquad_step, rbj_lowpass, Biquad, BiquadState};
use crate::dsp::DspStage;

/// Crossover points, Hz. Hiss lives in the top two bands; the bottom two
/// exist so a low threshold doesn't pump the bass.
const CROSSOVERS: [f64; 3] = [400.0, 2500.0, 8000.0];
/// Butterworth Q for the LR4 halves.
const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Downward expansion ratio below the threshold (v1 constant).
const RATIO: f64 = 2.0;
/// Soft knee around the threshold, dB (v1 constant).
const KNEE_DB: f64 = 6.0;
/// Enable/disable blend time (the crossfeed's ramp uses the same 15 ms).
const RAMP_SECONDS: f64 = 0.015;

/// User-facing settings, persisted in `engine-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DehissSettings {
    pub enabled: bool,
    /// Master threshold, dBFS.
    pub threshold_db: f32,
    /// Per-band threshold trim, dB: hiss-weighted toward the top bands.
    /// This array is what a future Learn function fills in.
    pub trim_db: [f32; 4],
    /// Maximum reduction per band, dB.
    pub amount_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Dry/wet blend: 0 = dry, 1 = fully processed. The A/B control.
    pub dry_wet: f32,
}

impl Default for DehissSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -50.0,
            trim_db: [0.0, 0.0, 3.0, 6.0],
            amount_db: 12.0,
            attack_ms: 1.0,
            release_ms: 100.0,
            dry_wet: 1.0,
        }
    }
}

impl DehissSettings {
    /// Pull every value into its allowed range; non-finite values become
    /// their defaults (same contract as `AnalogSettings::clamped`).
    pub fn clamped(mut self) -> Self {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let d = Self::default();
        self.threshold_db = fin(self.threshold_db, d.threshold_db).clamp(-80.0, -20.0);
        for (t, dt) in self.trim_db.iter_mut().zip(d.trim_db.iter()) {
            *t = fin(*t, *dt).clamp(-12.0, 12.0);
        }
        self.amount_db = fin(self.amount_db, d.amount_db).clamp(0.0, 24.0);
        self.attack_ms = fin(self.attack_ms, d.attack_ms).clamp(0.1, 20.0);
        self.release_ms = fin(self.release_ms, d.release_ms).clamp(20.0, 1000.0);
        self.dry_wet = fin(self.dry_wet, d.dry_wet).clamp(0.0, 1.0);
        self
    }
}

/// The de-hiss stage. See the module docs.
pub struct DehissStage {
    settings: DehissSettings,
    sample_rate: u32,
    /// LR4 crossover: per split, two cascaded Butterworth low-pass biquads.
    lp: [[Biquad; 2]; 3],
    /// Filter state: [split][cascade stage][channel].
    lp_state: [[[BiquadState; 2]; 2]; 3],
    /// Stereo-linked envelope per band (linear).
    env: [f64; 4],
    attack_c: f64,
    release_c: f64,
    /// Wet blend: 0 = dry, 1 = fully expanded. Ramps on toggle.
    mix: f64,
}

impl DehissStage {
    pub fn new(sample_rate: u32) -> Self {
        let mut s = Self {
            settings: DehissSettings::default(),
            sample_rate: 0, // designed by prepare() below
            lp: [[design_lp(CROSSOVERS[0], 1); 2]; 3],
            lp_state: [[[BiquadState::default(); 2]; 2]; 3],
            env: [0.0; 4],
            attack_c: 0.0,
            release_c: 0.0,
            mix: 0.0,
        };
        s.prepare(sample_rate);
        s
    }

    /// Apply new settings (clamped). Non-finite values become defaults;
    /// ranges are pulled into bounds — the previous settings are never kept
    /// on bad input, they are repaired.
    pub fn set_settings(&mut self, settings: DehissSettings) {
        self.settings = settings.clamped();
    }

    pub fn settings(&self) -> DehissSettings {
        self.settings
    }

    fn clear_state(&mut self) {
        self.lp_state = [[[BiquadState::default(); 2]; 2]; 3];
        self.env = [0.0; 4];
    }

    fn mix_target(&self) -> f64 {
        if self.settings.enabled {
            1.0
        } else {
            0.0
        }
    }

    /// Downward-expander gain for one band (linear, ≤ 1.0).
    fn band_gain(&self, env_db: f64, threshold_db: f64) -> f64 {
        let slope = 1.0 - 1.0 / RATIO;
        let gain_db = if env_db >= threshold_db + KNEE_DB / 2.0 {
            0.0
        } else if env_db <= threshold_db - KNEE_DB / 2.0 {
            ((env_db - threshold_db) * slope).max(-self.settings.amount_db as f64)
        } else {
            // Soft knee: quadratic blend, continuous in value and slope at
            // both junctions.
            let d = env_db - threshold_db - KNEE_DB / 2.0;
            (-slope * d * d / (2.0 * KNEE_DB)).max(-self.settings.amount_db as f64)
        };
        10f64.powf(gain_db / 20.0)
    }
}

/// Butterworth low-pass biquad (one LR4 half).
fn design_lp(freq_hz: f64, sample_rate: u32) -> Biquad {
    rbj_lowpass(freq_hz, BUTTERWORTH_Q, sample_rate as f64)
}

impl DspStage for DehissStage {
    fn prepare(&mut self, sample_rate: u32) {
        // Psychoacoustic constants are absolute frequencies, so the design
        // scales with the rate — no special-casing for high-res audio.
        self.sample_rate = sample_rate;
        for (s, &fc) in CROSSOVERS.iter().enumerate() {
            self.lp[s] = [design_lp(fc, sample_rate), design_lp(fc, sample_rate)];
        }
        let sr = sample_rate as f64;
        // One-pole ballistics: coefficient per sample for the ms time.
        self.attack_c = (-1.0 / (self.settings.attack_ms as f64 / 1000.0 * sr)).exp();
        self.release_c = (-1.0 / (self.settings.release_ms as f64 / 1000.0 * sr)).exp();
        // History from another rate would ring.
        self.clear_state();
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
        let step = 1.0 / (RAMP_SECONDS * self.sample_rate.max(1) as f64);
        let wet_target = self.settings.dry_wet as f64;

        for frame in interleaved.chunks_exact_mut(2) {
            let x = [frame[0] as f64, frame[1] as f64];

            // LR4 split, cascaded: low = LP(LP(remainder)),
            // high = remainder - low. The four bands sum back to x.
            let mut rem = x;
            let mut bands = [[0.0f64; 2]; 4];
            for (s, band) in bands.iter_mut().enumerate().take(3) {
                let mut low = [0.0f64; 2];
                for ch in 0..2 {
                    let y1 =
                        biquad_step(&self.lp[s][0], &mut self.lp_state[s][0][ch], rem[ch] as f32);
                    let y2 = biquad_step(&self.lp[s][1], &mut self.lp_state[s][1][ch], y1);
                    low[ch] = y2 as f64;
                }
                *band = low;
                rem = [rem[0] - low[0], rem[1] - low[1]];
            }
            bands[3] = rem;
            // Stereo-linked envelopes and gains.
            let mut gains = [1.0f64; 4];
            for b in 0..4 {
                let peak = bands[b][0].abs().max(bands[b][1].abs());
                let c = if peak > self.env[b] {
                    self.attack_c
                } else {
                    self.release_c
                };
                self.env[b] = peak + c * (self.env[b] - peak);
                let env_db = 20.0 * (self.env[b] + 1e-12).log10();
                let thr = self.settings.threshold_db as f64 + self.settings.trim_db[b] as f64;
                gains[b] = self.band_gain(env_db, thr);
            }

            let mut wet = [0.0f64; 2];
            for b in 0..4 {
                wet[0] += bands[b][0] * gains[b];
                wet[1] += bands[b][1] * gains[b];
            }

            let m = self.mix * wet_target;
            frame[0] = (x[0] + (wet[0] - x[0]) * m) as f32;
            frame[1] = (x[1] + (wet[1] - x[1]) * m) as f32;

            // Toward the target; snap at the ends so bypass stays exact.
            self.mix = if (target - self.mix).abs() <= step {
                target
            } else {
                self.mix + step * (target - self.mix).signum()
            };
        }
    }

    /// IIR crossovers have phase shift but add no block delay.
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

    fn stage(sample_rate: u32) -> DehissStage {
        let mut s = DehissStage::new(sample_rate);
        s.set_settings(DehissSettings {
            enabled: true,
            ..DehissSettings::default()
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

    #[test]
    fn bypass_is_bit_transparent() {
        let mut s = DehissStage::new(44100);
        let mut n = noise(7);
        let input = stereo(4096, |_| (n(), n()));
        let mut buf = input.clone();
        s.process(&mut buf, 2);
        assert_eq!(buf, input, "a disabled stage must not touch a sample");
    }

    #[test]
    fn silence_in_is_silence_out() {
        let mut s = stage(44100);
        let mut buf = stereo(8192, |_| (0.0, 0.0));
        s.process(&mut buf, 2);
        assert!(buf.iter().all(|x| *x == 0.0));
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    /// Steady hiss below threshold: the top bands hit the maximum
    /// reduction, the bottom bands (trim 0) less so.
    #[test]
    fn steady_hiss_is_pushed_down_in_the_top_bands() {
        let mut s = stage(44100);
        // Hiss floor at -75 dBFS: 31 dB under the top band's threshold
        // (-50 + 6), so the 2:1 expander clamps at amount_db = 12 dB.
        let level = 10f32.powf(-75.0 / 20.0);
        let mut n = noise(11);
        let mut buf = stereo(44100, |_| (n() * level, n() * level));
        s.process(&mut buf, 2);
        // Measure the top band's energy: high-pass the output hard and
        // compare against the input's.
        let tail = &buf[buf.len() / 2..];
        let rms_top = |samples: &[f32]| {
            // Crude top-band isolation: first difference emphasizes highs.
            let v: Vec<f64> = samples
                .iter()
                .skip(1)
                .step_by(2)
                .map(|x| *x as f64)
                .collect();
            let d: Vec<f64> = v.windows(2).map(|w| w[1] - w[0]).collect();
            (d.iter().map(|x| x * x).sum::<f64>() / d.len() as f64).sqrt()
        };
        let out_top = rms_top(tail);
        // Input top-band energy for comparison (same differencing).
        let mut n2 = noise(11);
        let input: Vec<f32> = (0..tail.len()).map(|_| n2() * level).collect();
        let in_top = rms_top(&input);
        let reduction_db = 20.0 * (out_top / in_top).log10();
        assert!(
            (reduction_db + 12.0).abs() < 2.0,
            "top band reduction {reduction_db:.1} dB, want ≈ -12 dB"
        );
    }

    /// A tone well above threshold passes at its level with no new
    /// harmonics: this stage creates no distortion.
    #[test]
    fn loud_tone_passes_untouched() {
        let mut s = stage(48000);
        let amp = 10f32.powf(-20.0 / 20.0);
        let mut buf = stereo(48000, |i| {
            let v = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48000.0).sin() as f32 * amp;
            (v, v)
        });
        s.process(&mut buf, 2);
        let tail = &buf[buf.len() / 2..];
        let rms_out = (tail
            .iter()
            .step_by(2)
            .map(|x| (*x as f64).powi(2))
            .sum::<f64>()
            / (tail.len() / 2) as f64)
            .sqrt();
        let want = (amp as f64) / std::f64::consts::SQRT_2;
        let diff_db = 20.0 * (rms_out / want).log10();
        assert!(diff_db.abs() < 0.2, "tone level changed by {diff_db:.2} dB");
    }

    /// Stereo linking: a loud tone on L only holds the gate open for both
    /// channels, so the image doesn't wander.
    #[test]
    fn stereo_linking_keeps_the_image_stable() {
        let mut s = stage(44100);
        let loud = 10f32.powf(-20.0 / 20.0);
        let quiet = 10f32.powf(-70.0 / 20.0);
        let mut n = noise(5);
        // L loud (above threshold), R quiet hiss (below).
        let input = stereo(44100, |_| (n() * loud, n() * quiet));
        let mut buf = input.clone();
        s.process(&mut buf, 2);
        let tail = &buf[buf.len() / 2..];
        let in_tail = &input[input.len() / 2..];
        let rms = |samples: &[f32], ch: usize| {
            (samples
                .iter()
                .skip(ch)
                .step_by(2)
                .map(|x| (*x as f64).powi(2))
                .sum::<f64>()
                / (samples.len() / 2) as f64)
                .sqrt()
        };
        // R must not be ducked: linked to L's envelope, gain ≈ 1, so the
        // output R RMS matches the input R RMS.
        let ratio = rms(tail, 1) / rms(in_tail, 1).max(1e-12);
        let diff_db = 20.0 * ratio.log10();
        assert!(
            diff_db.abs() < 3.0,
            "right channel moved by {diff_db:.1} dB despite the linked gate"
        );
    }

    #[test]
    fn reset_clears_the_followers() {
        let mut s = stage(44100);
        // Loud chunk first: followers ride high.
        let mut buf = stereo(8192, |_| (0.5, 0.5));
        s.process(&mut buf, 2);
        s.reset();
        // Then silence: full reduction immediately, no hangover.
        let mut buf = stereo(4096, |_| (0.0, 0.0));
        s.process(&mut buf, 2);
        assert!(buf.iter().all(|x| *x == 0.0));
        assert!(s.env.iter().all(|e| *e == 0.0));
    }

    #[test]
    fn latency_is_zero() {
        assert_eq!(stage(48000).latency_frames(), 0);
    }

    #[test]
    fn clamped_repairs_non_finite_and_bounds_ranges() {
        let bad = DehissSettings {
            threshold_db: f32::NAN,
            amount_db: 100.0,
            trim_db: [0.0, -50.0, 0.0, 99.0],
            ..DehissSettings::default()
        };
        let c = bad.clamped();
        assert_eq!(c.threshold_db, -50.0);
        assert_eq!(c.amount_db, 24.0);
        assert_eq!(c.trim_db[1], -12.0);
        assert_eq!(c.trim_db[3], 12.0);
    }

    #[test]
    fn settings_round_trip_and_old_files_default_to_off() {
        let s = DehissSettings {
            enabled: true,
            threshold_db: -60.0,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<DehissSettings>(&json).unwrap(), s);
        let partial: DehissSettings = serde_json::from_str(r#"{"threshold_db": -40.0}"#).unwrap();
        assert!(!partial.enabled);
        assert_eq!(partial.threshold_db, -40.0);
        assert_eq!(partial.trim_db, [0.0, 0.0, 3.0, 6.0]);
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
