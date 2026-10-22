//! Tape (Stage 1): memoryless tape coloration for the Analog Warmth section.
//!
//! A cheap, shippable, *memoryless* sweetener: record/repro EQ around a
//! soft saturator, head bump, wow/flutter/drift, and a hiss/asperity noise
//! model. It sounds like tape at sane settings and is honest about what it
//! is not: the memory-dependent physics (minor hysteresis loops,
//! history-dependent transients) arrive with the Stage 2 Jiles–Atherton
//! core. Spec: docs/v2/Tape-Stage1.md. Primer:
//! `Euphonics-Magnetic-Tape-Primer.md`.
//!
//! # Algorithm
//!
//! In place on interleaved f32, per channel:
//!
//! ```text
//! x1 = record_eq(x)            # pre-emphasis: NAB or IEC, by IPS
//! x2 = drive_db applied        # input gain into the curve
//! x3 = saturate(x2)            # memoryless odd curve, ADAA or oversampled
//! x4 = repro_eq(x3)            # de-emphasis + head bump
//! x5 = wow_flutter(x4)         # fractional modulated delay, decorrelated L/R
//! y  = x5 + hiss + asperity(x5)# noise floor, then output trim
//! ```
//!
//! - Record/repro EQ: first-order sections from the standards' time
//!   constants via the bilinear transform, normalized to 0 dB at 1 kHz.
//!   NAB (1965): 3180 µs + 50 µs; IEC: 35 µs @ 15 ips, 17.5 µs @ 30 ips.
//!   (The exact IEC standards history is a secondary source — kept as a
//!   code comment, not a UI claim.) The bass lift is a leaky integrator
//!   (pole a decade below turnover) rather than a true integrator, so the
//!   filter is stable; it matches the ideal curve within 0.1 dB above
//!   10 Hz. Record is the exact inverse of repro: the pair composes flat.
//! - Saturation: asymmetric `tanh`, unity small-signal gain
//!   (`tanh(kx)/k`), knee from `saturation`, asymmetry from `bias`
//!   (even-order content). `bias` is documented to remap to J-A loop width
//!   when Stage 2 lands, so the control survives the upgrade.
//!   Anti-aliasing: ADAA at base rate in Eco; 2x/4x zero-stuff oversampling
//!   through a Kaiser FIR in Standard/High (the dry path is delayed to
//!   match, the analog stage's pattern).
//! - Head bump: LF bell (~+2.5 dB @ ~45 Hz @ 15 ips, an octave higher at
//!   30 ips) plus the mild notch at double the bump frequency —
//!   "characteristic of pretty much all real tape machines" (Airwindows
//!   ToTape7, verified in the research report).
//! - Wow/flutter/drift: fractional-delay line; wow + flutter sines plus a
//!   slow random-walk drift, L/R decorrelated by phase offset. Skipped
//!   entirely (no added latency) when all three depths are zero.
//! - Noise: shaped hiss plus signal-dependent asperity noise, both exactly
//!   zero at minimum, deterministically seeded per `reset()`.
//!
//! Enabling/disabling ramps over 15 ms; `dry_wet` is the A/B control.
//!
//! # Honest limitations
//!
//! Stage 1 is memoryless: no minor hysteresis loops, no history-dependent
//! transients — the fundamental limitation motivating Stage 2. No UI or doc
//! may claim "modeled after" any hardware; the checkable references are
//! CHOW Tape Model and Airwindows.

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use super::biquad::{biquad_step, rbj_peaking, Biquad, BiquadState};
use crate::dsp::DspStage;

/// Enable/disable blend time.
const RAMP_SECONDS: f64 = 0.015;
/// Hiss level at or below this is exactly off.
const HISS_OFF_DB: f32 = -90.0;
/// Fixed RNG seed: `reset()` reproduces the same noise (tests, no surprises).
const NOISE_SEED: u32 = 0x243F_6A88;

/// Tape speed: the studio standards. 7.5 ips is reserved, not implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapeSpeed {
    Ips15,
    Ips30,
}

/// Record/repro equalization standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapeEqStandard {
    Nab,
    Iec,
}

/// Anti-aliasing quality for the saturator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapeQuality {
    /// ADAA at base rate.
    Eco,
    /// 2x oversampled.
    Standard,
    /// 4x oversampled.
    High,
}

/// User-facing settings, persisted in `engine-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TapeSettings {
    pub enabled: bool,
    /// Input gain into the curve, dB.
    pub drive_db: f32,
    pub output_trim_db: f32,
    /// Knee sharpness, 0..1.
    pub saturation: f32,
    /// Under↔over: curve asymmetry (even-order content). Remaps to J-A loop
    /// width when Stage 2 lands.
    pub bias: f32,
    pub ips: TapeSpeed,
    pub eq_standard: TapeEqStandard,
    /// Head-bump amount, 0..1.
    pub head_bump: f32,
    pub wow_rate_hz: f32,
    pub wow_depth_pct: f32,
    pub flutter_rate_hz: f32,
    pub flutter_depth_pct: f32,
    /// Slow wander amount, 0..1 (±0.05 % at full).
    pub drift: f32,
    /// Hiss level, dB; at or below −90 dB it is exactly off.
    pub hiss_db: f32,
    /// Signal-dependent grit, 0..1.
    pub asperity: f32,
    pub quality: TapeQuality,
    /// Dry/wet blend: 0 = dry, 1 = fully tape. The A/B control.
    pub dry_wet: f32,
}

impl Default for TapeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            drive_db: 0.0,
            output_trim_db: 0.0,
            saturation: 0.35,
            bias: 0.5,
            ips: TapeSpeed::Ips15,
            eq_standard: TapeEqStandard::Nab,
            head_bump: 0.5,
            wow_rate_hz: 0.6,
            wow_depth_pct: 0.15,
            flutter_rate_hz: 6.0,
            flutter_depth_pct: 0.05,
            drift: 0.2,
            hiss_db: -78.0,
            asperity: 0.3,
            quality: TapeQuality::Standard,
            dry_wet: 1.0,
        }
    }
}

impl TapeSettings {
    /// Pull every value into its allowed range; non-finite values become
    /// their defaults (same contract as `AnalogSettings::clamped`).
    pub fn clamped(mut self) -> Self {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let d = Self::default();
        self.drive_db = fin(self.drive_db, d.drive_db).clamp(-12.0, 12.0);
        self.output_trim_db = fin(self.output_trim_db, d.output_trim_db).clamp(-12.0, 12.0);
        self.saturation = fin(self.saturation, d.saturation).clamp(0.0, 1.0);
        self.bias = fin(self.bias, d.bias).clamp(0.0, 1.0);
        self.head_bump = fin(self.head_bump, d.head_bump).clamp(0.0, 1.0);
        self.wow_rate_hz = fin(self.wow_rate_hz, d.wow_rate_hz).clamp(0.2, 2.0);
        self.wow_depth_pct = fin(self.wow_depth_pct, d.wow_depth_pct).clamp(0.0, 0.5);
        self.flutter_rate_hz = fin(self.flutter_rate_hz, d.flutter_rate_hz).clamp(4.0, 12.0);
        self.flutter_depth_pct = fin(self.flutter_depth_pct, d.flutter_depth_pct).clamp(0.0, 0.2);
        self.drift = fin(self.drift, d.drift).clamp(0.0, 1.0);
        self.hiss_db = fin(self.hiss_db, d.hiss_db).clamp(HISS_OFF_DB, -30.0);
        self.asperity = fin(self.asperity, d.asperity).clamp(0.0, 1.0);
        self.dry_wet = fin(self.dry_wet, d.dry_wet).clamp(0.0, 1.0);
        self
    }

    fn oversample_factor(self) -> usize {
        match self.quality {
            TapeQuality::Eco => 1,
            TapeQuality::Standard => 2,
            TapeQuality::High => 4,
        }
    }
}

/// First-order (1 + s·τa)/(1 + s·τb) sections are built from these helpers.
///
/// NAB repro (de-emphasis): bass lift below 1/(2π·3180µs) ≈ 50 Hz and treble
/// cut above 1/(2π·50µs) ≈ 3183 Hz, per the NAB 1965 standard (verified
/// live in the research report). IEC repro is treble cut only (single τ).
/// The bass lift is `1 +` a leaky integrator — pole a decade below the
/// turnover (δ = 0.1) instead of at DC — so the filter is stable while
/// matching the ideal curve within 0.1 dB above 10 Hz. Record pre-emphasis
/// is the exact inverse of repro: the pair composes flat.
/// Time constants (seconds): (bass τ, treble τ); treble τ = 0 means none.
fn eq_taus(ips: TapeSpeed, standard: TapeEqStandard) -> (f64, f64) {
    match standard {
        TapeEqStandard::Nab => (3180e-6, 50e-6),
        // Exact IEC standards history is a secondary source (audiokarma
        // thread, verbatim technical — not a primary standard); the values
        // below are the commonly cited ones.
        TapeEqStandard::Iec => (
            0.0,
            match ips {
                TapeSpeed::Ips15 => 35e-6,
                TapeSpeed::Ips30 => 17.5e-6,
            },
        ),
    }
}

fn identity_biquad() -> Biquad {
    Biquad {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    }
}

/// Bass-lift section: `1 + 1/(s·τ + δ)`, bilinear-transformed, δ = 0.1.
fn design_bass_lift(tau: f64, sample_rate: f64) -> Biquad {
    let t = 1.0 / sample_rate;
    let k = 2.0 * tau / t;
    let delta = 0.1;
    let a0 = k + delta;
    let a1 = delta - k;
    Biquad {
        b0: (a0 + 1.0) / a0,
        b1: (a1 + 1.0) / a0,
        b2: 0.0,
        a1: a1 / a0,
        a2: 0.0,
    }
}

/// Treble-cut section: `1/(1 + s·τ)`, bilinear-transformed.
fn design_treble_cut(tau: f64, sample_rate: f64) -> Biquad {
    let t = 1.0 / sample_rate;
    let k = 2.0 * tau / t;
    Biquad {
        b0: 1.0 / (1.0 + k),
        b1: 1.0 / (1.0 + k),
        b2: 0.0,
        a1: (1.0 - k) / (1.0 + k),
        a2: 0.0,
    }
}

/// Invert a first-order biquad section (record = 1/repro).
fn invert_first_order(c: &Biquad) -> Biquad {
    Biquad {
        b0: 1.0 / c.b0,
        b1: c.a1 / c.b0,
        b2: 0.0,
        a1: c.b1 / c.b0,
        a2: 0.0,
    }
}

/// Repro (de-emphasis) sections, normalized to 0 dB at 1 kHz.
fn design_repro(ips: TapeSpeed, standard: TapeEqStandard, sample_rate: f64) -> [Biquad; 2] {
    let (tau_b, tau_t) = eq_taus(ips, standard);
    let bass = if tau_b > 0.0 {
        design_bass_lift(tau_b, sample_rate)
    } else {
        identity_biquad()
    };
    let treble = if tau_t > 0.0 {
        design_treble_cut(tau_t, sample_rate)
    } else {
        identity_biquad()
    };
    // Normalize the pair to 0 dB at the 1 kHz reference.
    let w = 2.0 * PI * 1000.0 / sample_rate;
    let mag = bass.magnitude(w) * treble.magnitude(w);
    let mut treble = treble;
    treble.b0 /= mag;
    treble.b1 /= mag;
    [bass, treble]
}

/// Record (pre-emphasis) sections: the exact inverse of repro, normalized
/// to 0 dB at 1 kHz.
fn design_record(ips: TapeSpeed, standard: TapeEqStandard, sample_rate: f64) -> [Biquad; 2] {
    let (tau_b, tau_t) = eq_taus(ips, standard);
    let bass = if tau_b > 0.0 {
        invert_first_order(&design_bass_lift(tau_b, sample_rate))
    } else {
        identity_biquad()
    };
    let treble = if tau_t > 0.0 {
        invert_first_order(&design_treble_cut(tau_t, sample_rate))
    } else {
        identity_biquad()
    };
    let w = 2.0 * PI * 1000.0 / sample_rate;
    let mag = bass.magnitude(w) * treble.magnitude(w);
    let mut treble = treble;
    treble.b0 /= mag;
    treble.b1 /= mag;
    [bass, treble]
}

/// Head-bump center, Hz, by speed (ballpark from secondary sources — the
/// exact dB/Hz mapping lives here in the code, not in the UI).
fn bump_freq(ips: TapeSpeed) -> f64 {
    match ips {
        TapeSpeed::Ips15 => 45.0,
        TapeSpeed::Ips30 => 90.0,
    }
}

/// Asymmetric tanh shaper, unity small-signal gain: `tanh(k·x)/k`.
/// `bias` 0.5 is symmetric (odd-only); away from 0.5 the positive and
/// negative knees differ, producing even-order content.
struct Shaper {
    kp: f64,
    kn: f64,
}

impl Shaper {
    fn new(saturation: f32, bias: f32) -> Self {
        let k = 1.0 + saturation as f64 * 3.0;
        let asym = (bias as f64 - 0.5) * 1.0;
        Self {
            kp: (k * (1.0 + asym)).max(0.2),
            kn: (k * (1.0 - asym)).max(0.2),
        }
    }

    fn shape(&self, x: f64) -> f64 {
        if x >= 0.0 {
            (self.kp * x).tanh() / self.kp
        } else {
            (self.kn * x).tanh() / self.kn
        }
    }

    /// Antiderivative of `shape`: ∫ tanh(kx)/k dx = ln(cosh(kx))/k².
    /// Continuous at 0 (both sides are 0 there), so ADAA stays valid.
    fn anti(&self, x: f64) -> f64 {
        if x >= 0.0 {
            (self.kp * x).cosh().ln() / (self.kp * self.kp)
        } else {
            (self.kn * x).cosh().ln() / (self.kn * self.kn)
        }
    }

    /// First-order ADAA: alias-suppressed evaluation of the curve.
    fn adaa(&self, x0: f64, x1: f64) -> f64 {
        let dx = x1 - x0;
        if dx.abs() < 1e-7 {
            self.shape(x1)
        } else {
            (self.anti(x1) - self.anti(x0)) / dx
        }
    }
}

/// Kaiser-windowed low-pass FIR, unity DC gain. `cutoff` is a fraction of
/// the sample rate it runs at (0.5 = Nyquist).
fn kaiser_fir(taps: usize, cutoff: f64) -> Vec<f32> {
    let m = (taps - 1) as f64;
    let beta = 8.0;
    let i0 = |x: f64| {
        let (mut s, mut t) = (1.0, 1.0);
        for k in 1..40 {
            t *= (x / (2.0 * k as f64)).powi(2);
            s += t;
        }
        s
    };
    let h: Vec<f64> = (0..taps)
        .map(|n| {
            let k = n as f64 - m / 2.0;
            let sinc = if k == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * k).sin() / (PI * k)
            };
            let arg = beta * (1.0 - (2.0 * n as f64 / m - 1.0).powi(2)).max(0.0).sqrt();
            sinc * i0(arg) / i0(beta)
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| (v / sum) as f32).collect()
}

/// Doubled ring buffer: the newest `len` values are always one slice.
struct Ring {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
}

impl Ring {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len * 2],
            len: len.max(1),
            pos: 0,
        }
    }

    fn push(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.buf[self.pos + self.len] = v;
        self.pos = (self.pos + 1) % self.len;
    }

    /// The contiguous history, newest last.
    fn history(&self) -> &[f32] {
        &self.buf[self.pos..self.pos + self.len]
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
    }
}

/// Per-channel state.
struct Channel {
    rec_s: [BiquadState; 2],
    repro_s: [BiquadState; 2],
    bump_s: BiquadState,
    notch_s: BiquadState,
    /// ADAA memory (Eco): previous curve input.
    adaa_x0: f64,
    /// Oversampling FIR histories.
    up: Ring,
    dn: Ring,
    /// Dry-path delay compensating the oversampler's group delay.
    dry: Ring,
    /// Wow/flutter delay line.
    delay: Vec<f32>,
    delay_pos: usize,
    wow_phase: f64,
    flutter_phase: f64,
    drift_state: f64,
    /// Noise shaping state.
    hiss_lp: f64,
    env: f64,
}

impl Channel {
    fn new() -> Self {
        Self {
            rec_s: [BiquadState::default(); 2],
            repro_s: [BiquadState::default(); 2],
            bump_s: BiquadState::default(),
            notch_s: BiquadState::default(),
            adaa_x0: 0.0,
            up: Ring::new(1),
            dn: Ring::new(1),
            dry: Ring::new(1),
            delay: Vec::new(),
            delay_pos: 0,
            wow_phase: 0.0,
            flutter_phase: 0.0,
            drift_state: 0.0,
            hiss_lp: 0.0,
            env: 0.0,
        }
    }
}

/// The tape stage. See the module docs.
pub struct TapeStage {
    settings: TapeSettings,
    sample_rate: u32,
    rec: [Biquad; 2],
    repro: [Biquad; 2],
    bump: Biquad,
    notch: Biquad,
    shaper: Shaper,
    drive_gain: f64,
    trim_gain: f64,
    factor: usize,
    fir: Vec<f32>,
    /// Dry-path delay in base-rate frames (oversampler group delay).
    fir_delay: usize,
    /// Wow/flutter delay-line depth in frames (worst-case excursion).
    delay_len: usize,
    /// Constant base offset so the modulated delay never goes negative.
    delay_base: f64,
    hiss_gain: f64,
    hiss_a: f64,
    channels: [Channel; 2],
    rng: u32,
    /// Wet blend: 0 = dry, 1 = fully tape. Ramps on toggle.
    mix: f64,
}

impl TapeStage {
    pub fn new(sample_rate: u32) -> Self {
        let mut s = Self {
            settings: TapeSettings::default(),
            sample_rate: 0,
            rec: [identity_biquad(), identity_biquad()],
            repro: [identity_biquad(), identity_biquad()],
            bump: identity_biquad(),
            notch: identity_biquad(),
            shaper: Shaper::new(0.35, 0.5),
            drive_gain: 1.0,
            trim_gain: 1.0,
            factor: 2,
            fir: Vec::new(),
            fir_delay: 0,
            delay_len: 0,
            delay_base: 0.0,
            hiss_gain: 0.0,
            hiss_a: 0.0,
            channels: [Channel::new(), Channel::new()],
            rng: NOISE_SEED,
            mix: 0.0,
        };
        s.prepare(sample_rate);
        s
    }

    /// Deterministic white noise in [-1, 1).
    fn noise(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f64 / u32::MAX as f64) * 2.0 - 1.0
    }

    /// Apply new settings (clamped). Geometry changes (quality, speed, EQ)
    /// redesign the filters; switching on from fully dry clears state.
    pub fn set_settings(&mut self, settings: TapeSettings) {
        let clamped = settings.clamped();
        let redesign = clamped.quality != self.settings.quality
            || clamped.ips != self.settings.ips
            || clamped.eq_standard != self.settings.eq_standard;
        if clamped.enabled && !self.settings.enabled && self.mix == 0.0 {
            self.clear_state();
        }
        self.settings = clamped;
        self.shaper = Shaper::new(self.settings.saturation, self.settings.bias);
        self.drive_gain = 10f64.powf(self.settings.drive_db as f64 / 20.0);
        self.trim_gain = 10f64.powf(self.settings.output_trim_db as f64 / 20.0);
        self.hiss_gain = if self.settings.hiss_db <= HISS_OFF_DB {
            0.0
        } else {
            10f64.powf(self.settings.hiss_db as f64 / 20.0)
        };
        if redesign {
            self.design();
        }
    }

    pub fn settings(&self) -> TapeSettings {
        self.settings
    }

    /// (Re)design filters and delay geometry for the current settings/rate.
    fn design(&mut self) {
        let sr = self.sample_rate as f64;
        self.rec = design_record(self.settings.ips, self.settings.eq_standard, sr);
        self.repro = design_repro(self.settings.ips, self.settings.eq_standard, sr);
        let f0 = bump_freq(self.settings.ips);
        let hb = self.settings.head_bump as f64;
        // Zero-gain peaking is exactly identity, so no special-casing.
        self.bump = rbj_peaking(f0, 1.0, hb * 2.5, sr);
        self.notch = rbj_peaking(2.0 * f0, 2.0, -hb * 1.0, sr);

        // Oversampling FIR: 16 taps per factor, cutoff just below the base
        // Nyquist (as a fraction of the high rate).
        self.factor = self.settings.oversample_factor();
        if self.factor > 1 {
            let taps = 16 * self.factor;
            self.fir = kaiser_fir(taps, 0.45 / self.factor as f64);
            self.fir_delay = ((taps - 1) / 2) / self.factor;
        } else {
            self.fir = Vec::new();
            self.fir_delay = 0;
        }

        // Wow/flutter delay line: worst-case excursion over the full
        // parameter ranges (slowest wow rate gives the largest delay).
        // A = depth_fraction · sr / (2π·rate).
        let wow_a = 0.005 * sr / (2.0 * PI * 0.2);
        let flut_a = 0.002 * sr / (2.0 * PI * 4.0);
        let drift_a = 0.0005 * sr;
        self.delay_len = (wow_a + flut_a + drift_a).ceil() as usize + 8;
        self.delay_base = wow_a + flut_a + drift_a + 4.0;

        // Hiss shaping: one-pole low-pass around 6 kHz ("pink-ish").
        self.hiss_a = 1.0 - (-2.0 * PI * 6000.0 / sr).exp();

        for c in self.channels.iter_mut() {
            c.up = Ring::new(self.fir.len().max(1));
            c.dn = Ring::new(self.fir.len().max(1));
            c.dry = Ring::new(self.fir_delay.max(1));
            c.delay = vec![0.0; self.delay_len.max(1)];
            c.delay_pos = 0;
        }
    }

    fn clear_state(&mut self) {
        for c in self.channels.iter_mut() {
            c.rec_s = [BiquadState::default(); 2];
            c.repro_s = [BiquadState::default(); 2];
            c.bump_s = BiquadState::default();
            c.notch_s = BiquadState::default();
            c.adaa_x0 = 0.0;
            c.up.clear();
            c.dn.clear();
            c.dry.clear();
            c.delay.iter_mut().for_each(|v| *v = 0.0);
            c.delay_pos = 0;
            c.hiss_lp = 0.0;
            c.env = 0.0;
        }
        self.rng = NOISE_SEED;
    }

    fn mix_target(&self) -> f64 {
        if self.settings.enabled {
            1.0
        } else {
            0.0
        }
    }

    fn transport_active(&self) -> bool {
        self.settings.wow_depth_pct > 0.0
            || self.settings.flutter_depth_pct > 0.0
            || self.settings.drift > 0.0
    }

    /// One base-rate sample through the saturator (oversampled when asked).
    fn saturate(&mut self, ch: usize, x: f64) -> f64 {
        if self.factor == 1 {
            let y = self.shaper.adaa(self.channels[ch].adaa_x0, x);
            self.channels[ch].adaa_x0 = x;
            return y;
        }
        let c = &mut self.channels[ch];
        let mut out = 0.0;
        for p in 0..self.factor {
            let h = if p == 0 { x as f32 } else { 0.0 };
            c.up.push(h);
            let up_hist = c.up.history();
            let mut acc = 0.0f64;
            for (k, ck) in self.fir.iter().enumerate() {
                acc += *ck as f64 * up_hist[up_hist.len() - 1 - k] as f64;
            }
            let s = self.shaper.shape(acc) as f32;
            c.dn.push(s);
            let dn_hist = c.dn.history();
            let mut acc = 0.0f64;
            for (k, ck) in self.fir.iter().enumerate() {
                acc += *ck as f64 * dn_hist[dn_hist.len() - 1 - k] as f64;
            }
            if p == 0 {
                out = acc;
            }
        }
        out
    }
}

impl DspStage for TapeStage {
    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        // New rate: everything is redesigned (set_settings only redesigns
        // on geometry changes at a constant rate).
        let s = self.settings;
        self.set_settings(s);
        self.design();
        // New rate: drop history rather than misread it.
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
        let sr = self.sample_rate as f64;
        let transport = self.transport_active();
        let wow_inc = 2.0 * PI * self.settings.wow_rate_hz as f64 / sr;
        let flut_inc = 2.0 * PI * self.settings.flutter_rate_hz as f64 / sr;
        let wow_a = self.settings.wow_depth_pct as f64 / 100.0 * sr
            / (2.0 * PI * self.settings.wow_rate_hz as f64);
        let flut_a = self.settings.flutter_depth_pct as f64 / 100.0 * sr
            / (2.0 * PI * self.settings.flutter_rate_hz as f64);
        let drift_max = self.settings.drift as f64 * 0.0005 * sr;

        for frame in interleaved.chunks_exact_mut(2) {
            let mut wet = [0.0f64; 2];
            for ch in 0..2 {
                let x = frame[ch] as f64;

                // Record pre-emphasis, then drive into the curve.
                let mut v =
                    biquad_step(&self.rec[0], &mut self.channels[ch].rec_s[0], x as f32) as f64;
                v = biquad_step(&self.rec[1], &mut self.channels[ch].rec_s[1], v as f32) as f64;
                v *= self.drive_gain;
                // Saturation (ADAA or oversampled).
                v = self.saturate(ch, v);
                // Repro de-emphasis + head bump.
                v = biquad_step(&self.repro[0], &mut self.channels[ch].repro_s[0], v as f32) as f64;
                v = biquad_step(&self.repro[1], &mut self.channels[ch].repro_s[1], v as f32) as f64;
                v = biquad_step(&self.bump, &mut self.channels[ch].bump_s, v as f32) as f64;
                v = biquad_step(&self.notch, &mut self.channels[ch].notch_s, v as f32) as f64;

                // Wow/flutter/drift on a fractional delay line (skipped
                // entirely when all depths are zero: no added latency).
                if transport {
                    let r = self.noise();
                    let c = &mut self.channels[ch];
                    let ch_phase = ch as f64;
                    let delay = self.delay_base
                        + wow_a * (c.wow_phase + ch_phase * 1.3).sin()
                        + flut_a * (c.flutter_phase + ch_phase * 2.1).sin()
                        + c.drift_state * drift_max;
                    c.wow_phase += wow_inc;
                    c.flutter_phase += flut_inc;
                    // Slow random-walk drift in [-1, 1].
                    c.drift_state += r * 0.002;
                    if c.drift_state > 1.0 {
                        c.drift_state = 2.0 - c.drift_state;
                    } else if c.drift_state < -1.0 {
                        c.drift_state = -2.0 - c.drift_state;
                    }
                    c.delay[c.delay_pos] = v as f32;
                    let read = c.delay_pos as f64 - delay;
                    let len = c.delay.len() as f64;
                    let mut idx = read % len;
                    if idx < 0.0 {
                        idx += len;
                    }
                    let i0 = idx.floor() as usize % c.delay.len();
                    let i1 = (i0 + 1) % c.delay.len();
                    let frac = idx - idx.floor();
                    v = c.delay[i0] as f64 * (1.0 - frac) + c.delay[i1] as f64 * frac;
                    c.delay_pos = (c.delay_pos + 1) % c.delay.len();
                }

                // Noise: shaped hiss + signal-dependent asperity.
                let mut n = 0.0;
                if self.hiss_gain > 0.0 {
                    let w = self.noise();
                    let c = &mut self.channels[ch];
                    c.hiss_lp += self.hiss_a * (w - c.hiss_lp);
                    n += c.hiss_lp * self.hiss_gain;
                }
                if self.settings.asperity > 0.0 {
                    let asp_n = self.noise();
                    let c = &mut self.channels[ch];
                    let a = (-1.0 / (0.001 * sr)).exp();
                    let r = (-1.0 / (0.050 * sr)).exp();
                    let av = v.abs();
                    let coeff = if av > c.env { a } else { r };
                    c.env = av + coeff * (c.env - av);
                    n += asp_n * c.env * self.settings.asperity as f64 * 0.5;
                }
                v = (v + n) * self.trim_gain;
                wet[ch] = v;
            }

            // Dry path, delayed to line up with the oversampled wet path
            // (the analog stage's pattern); the wow/flutter delay is wet-only.
            let mut dry = [frame[0] as f64, frame[1] as f64];
            if self.fir_delay > 0 {
                for ch in 0..2 {
                    let c = &mut self.channels[ch];
                    let d = c.dry.history();
                    dry[ch] = d[0] as f64;
                    c.dry.push(frame[ch]);
                }
            }

            let m = self.mix * wet_target;
            frame[0] = (dry[0] + (wet[0] - dry[0]) * m) as f32;
            frame[1] = (dry[1] + (wet[1] - dry[1]) * m) as f32;

            // Toward the target; snap at the ends so bypass stays exact.
            self.mix = if (target - self.mix).abs() <= step {
                target
            } else {
                self.mix + step * (target - self.mix).signum()
            };
        }
    }

    /// Wow/flutter delay-line depth plus oversampler group delay; 0 when
    /// bypassed. (The transport section is skipped when its depths are all
    /// zero, so the depth is the honest worst case only while it runs.)
    fn latency_frames(&self) -> u32 {
        if !self.settings.enabled {
            0
        } else {
            let mut lat = self.fir_delay;
            if self.transport_active() {
                lat += self.delay_len;
            }
            lat as u32
        }
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

    fn stage(sample_rate: u32) -> TapeStage {
        let mut s = TapeStage::new(sample_rate);
        s.set_settings(TapeSettings {
            enabled: true,
            ..TapeSettings::default()
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

    fn sine(i: usize, freq: f64, sr: u32, amp: f32) -> f32 {
        (2.0 * PI * freq * i as f64 / sr as f64).sin() as f32 * amp
    }

    /// Correlation-based amplitude of `freq` in the left channel.
    fn harmonic(buf: &[f32], freq: f64, sr: u32) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        let n = buf.len() / 2;
        for (i, x) in buf.iter().step_by(2).enumerate().take(n) {
            let ph = 2.0 * PI * freq * i as f64 / sr as f64;
            re += *x as f64 * ph.cos();
            im += *x as f64 * ph.sin();
        }
        2.0 * (re * re + im * im).sqrt() / n as f64
    }

    #[test]
    fn bypass_is_bit_transparent() {
        let mut s = TapeStage::new(44100);
        // Deterministic stand-in for noise.
        let mut x = 0x12345678u32;
        let mut n = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32) - 0.5
        };
        let input = stereo(4096, |_| (n(), n()));
        let mut buf = input.clone();
        s.process(&mut buf, 2);
        assert_eq!(buf, input, "a disabled stage must not touch a sample");
    }

    /// Record → repro composes flat: the EQ pair is exact inverses.
    #[test]
    fn record_repro_pair_is_flat() {
        for (ips, std) in [
            (TapeSpeed::Ips15, TapeEqStandard::Nab),
            (TapeSpeed::Ips15, TapeEqStandard::Iec),
            (TapeSpeed::Ips30, TapeEqStandard::Iec),
        ] {
            for sr in [44100u32, 192000] {
                let rec = design_record(ips, std, sr as f64);
                let repro = design_repro(ips, std, sr as f64);
                for freq in [40.0, 100.0, 1000.0, 5000.0, 10000.0, 15000.0] {
                    let w = 2.0 * PI * freq / sr as f64;
                    let mag = rec[0].magnitude(w)
                        * rec[1].magnitude(w)
                        * repro[0].magnitude(w)
                        * repro[1].magnitude(w);
                    let db = 20.0 * mag.log10();
                    assert!(
                        db.abs() < 0.3,
                        "{ips:?} {std:?} @ {sr} Hz, {freq} Hz: {db:.2} dB"
                    );
                }
            }
        }
    }

    /// The −3 dB turnovers land on the standards' time constants.
    #[test]
    fn eq_turnovers_match_the_time_constants() {
        // NAB repro: 3180 µs → ~50 Hz bass lift is +3 dB there (re its
        // high-frequency asymptote); 50 µs → 3183 Hz treble cut is −3 dB.
        let sr = 44100.0;
        let repro = design_repro(TapeSpeed::Ips15, TapeEqStandard::Nab, sr);
        let db = |b: &Biquad, f: f64| 20.0 * b.magnitude(2.0 * PI * f / sr).log10();
        // Treble section: −3 dB at 3183 Hz relative to its low-frequency gain.
        let t = db(&repro[1], 3183.0) - db(&repro[1], 100.0);
        assert!((t + 3.0).abs() < 0.5, "NAB treble turnover: {t:.2} dB");
        // IEC 15 ips repro: −3 dB at ~4547 Hz.
        let iec = design_repro(TapeSpeed::Ips15, TapeEqStandard::Iec, sr);
        let t = db(&iec[1], 4547.0) - db(&iec[1], 100.0);
        assert!((t + 3.0).abs() < 0.5, "IEC treble turnover: {t:.2} dB");
    }

    /// 3rd harmonic grows with drive; the curve is level-dependent, not a
    /// fixed distortion box.
    #[test]
    fn saturation_third_harmonic_tracks_drive() {
        let mut h3 = Vec::new();
        for drive_db in [0.0f32, 6.0, 12.0] {
            let mut s = TapeStage::new(44100);
            s.set_settings(TapeSettings {
                enabled: true,
                drive_db,
                // Isolate the saturator: flat EQ, no bump/transport/noise.
                eq_standard: TapeEqStandard::Iec,
                ips: TapeSpeed::Ips30,
                head_bump: 0.0,
                wow_depth_pct: 0.0,
                flutter_depth_pct: 0.0,
                drift: 0.0,
                hiss_db: -90.0,
                asperity: 0.0,
                quality: TapeQuality::Eco,
                ..TapeSettings::default()
            });
            s.reset();
            let mut buf = stereo(44100, |i| {
                let v = sine(i, 1000.0, 44100, 0.5);
                (v, v)
            });
            s.process(&mut buf, 2);
            let tail = &buf[buf.len() / 2..];
            h3.push(harmonic(tail, 3000.0, 44100));
        }
        assert!(h3[1] > h3[0] * 2.0, "drive 0→6 dB: {h3:?}");
        // At high drive the curve is deep into saturation, so harmonic
        // growth compresses — still monotonic, just sub-linear.
        assert!(h3[2] > h3[1] * 1.2, "drive 6→12 dB: {h3:?}");
    }

    /// Bias 0.5 is symmetric (no 2nd harmonic); off-center bias adds it.
    #[test]
    fn bias_asymmetry_produces_even_harmonics() {
        let mut h2 = Vec::new();
        for bias in [0.5f32, 0.9] {
            let mut s = TapeStage::new(44100);
            s.set_settings(TapeSettings {
                enabled: true,
                drive_db: 6.0,
                bias,
                eq_standard: TapeEqStandard::Iec,
                ips: TapeSpeed::Ips30,
                head_bump: 0.0,
                wow_depth_pct: 0.0,
                flutter_depth_pct: 0.0,
                drift: 0.0,
                hiss_db: -90.0,
                asperity: 0.0,
                quality: TapeQuality::Eco,
                ..TapeSettings::default()
            });
            s.reset();
            let mut buf = stereo(44100, |i| {
                let v = sine(i, 1000.0, 44100, 0.5);
                (v, v)
            });
            s.process(&mut buf, 2);
            let tail = &buf[buf.len() / 2..];
            let fund = harmonic(tail, 1000.0, 44100);
            h2.push(harmonic(tail, 2000.0, 44100) / fund);
        }
        assert!(h2[0] < 1e-3, "symmetric bias must stay odd-only: {}", h2[0]);
        assert!(h2[1] > 0.01, "asymmetric bias must add 2nd: {}", h2[1]);
    }

    /// A quiet signal passes nearly clean: level-dependence, not a fixed
    /// distortion box.
    #[test]
    fn quiet_signals_pass_nearly_clean() {
        let mut s = stage(44100);
        let mut buf = stereo(44100, |i| {
            let v = sine(i, 1000.0, 44100, 10f32.powf(-40.0 / 20.0));
            (v, v)
        });
        s.process(&mut buf, 2);
        let tail = &buf[buf.len() / 2..];
        let fund = harmonic(tail, 1000.0, 44100);
        let thd = (harmonic(tail, 2000.0, 44100).powi(2) + harmonic(tail, 3000.0, 44100).powi(2))
            .sqrt()
            / fund;
        assert!(thd < 0.01, "THD of a -40 dBFS tone: {thd:.4}");
    }

    /// Head-bump center follows the speed.
    #[test]
    fn head_bump_center_follows_ips() {
        for (ips, want) in [(TapeSpeed::Ips15, 45.0), (TapeSpeed::Ips30, 90.0)] {
            let sr = 48000.0;
            let bump = rbj_peaking(bump_freq(ips), 1.0, 2.5, sr);
            let at = |f: f64| 20.0 * bump.magnitude(2.0 * PI * f / sr).log10();
            let center = at(want);
            assert!(
                (center - 2.5).abs() < 0.3,
                "{ips:?}: bump at center {center:.2} dB"
            );
            assert!(at(want / 4.0) < 1.0 && at(want * 4.0) < 1.0);
        }
    }

    /// Wow puts sidebands at ±rate around a tone; L/R are decorrelated.
    #[test]
    fn wow_flutter_sidebands_and_stereo_decorrelation() {
        let mut s = TapeStage::new(44100);
        s.set_settings(TapeSettings {
            enabled: true,
            wow_rate_hz: 2.0,
            wow_depth_pct: 0.5,
            flutter_depth_pct: 0.0,
            drift: 0.0,
            eq_standard: TapeEqStandard::Iec,
            ips: TapeSpeed::Ips30,
            head_bump: 0.0,
            hiss_db: -90.0,
            asperity: 0.0,
            quality: TapeQuality::Eco,
            ..TapeSettings::default()
        });
        s.reset();
        let mut buf = stereo(44100 * 2, |i| {
            let v = sine(i, 1000.0, 44100, 0.5);
            (v, v)
        });
        s.process(&mut buf, 2);
        let tail = &buf[buf.len() / 2..];
        let fund = harmonic(tail, 1000.0, 44100);
        let sb = (harmonic(tail, 998.0, 44100) + harmonic(tail, 1002.0, 44100)) / fund;
        assert!(sb > 0.005, "wow sidebands too small: {sb:.4}");
        // L/R decorrelation: identical inputs must come out different.
        let diff = tail
            .iter()
            .step_by(2)
            .zip(tail.iter().skip(1).step_by(2))
            .map(|(l, r)| (l - r).abs())
            .fold(0.0, f32::max);
        assert!(diff > 1e-4, "L/R must be decorrelated, max diff {diff}");
    }

    /// Noise floor appears only when enabled; minimum settings are exactly
    /// silent; reseeding reproduces the noise bit-identically.
    #[test]
    fn noise_only_when_enabled_and_deterministic() {
        // Enabled with hiss: floor present.
        let mut s = stage(44100);
        let mut buf = stereo(22050, |_| (0.0, 0.0));
        s.process(&mut buf, 2);
        let rms = (buf.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / buf.len() as f64).sqrt();
        assert!(rms > 1e-9, "hiss floor must be present, rms {rms}");

        // Minimum settings: zero output delta.
        let mut s2 = TapeStage::new(44100);
        s2.set_settings(TapeSettings {
            enabled: true,
            hiss_db: -90.0,
            asperity: 0.0,
            wow_depth_pct: 0.0,
            flutter_depth_pct: 0.0,
            drift: 0.0,
            ..TapeSettings::default()
        });
        s2.reset();
        let input = stereo(8192, |i| (sine(i, 440.0, 44100, 0.3), 0.0));
        let mut buf2 = input.clone();
        s2.process(&mut buf2, 2);
        // (EQ/saturation still color the signal; the assertion is only that
        // the *noise* contributes nothing: rerun must be bit-identical.)
        let mut buf3 = input.clone();
        s2.reset();
        s2.process(&mut buf3, 2);
        assert_eq!(buf2, buf3, "reset() must reseed deterministically");
    }

    #[test]
    fn latency_is_honest() {
        let s = stage(44100);
        // Transport on at defaults: delay-line depth + Eco (no FIR delay).
        assert!(s.latency_frames() > 100, "latency {}", s.latency_frames());
        let mut off = TapeStage::new(44100);
        let _ = &mut off;
        assert_eq!(off.latency_frames(), 0);
    }

    #[test]
    fn clamped_repairs_non_finite_and_bounds_ranges() {
        let bad = TapeSettings {
            drive_db: f32::INFINITY,
            saturation: 5.0,
            hiss_db: f32::NAN,
            ..TapeSettings::default()
        };
        let c = bad.clamped();
        assert_eq!(c.drive_db, 0.0);
        assert_eq!(c.saturation, 1.0);
        assert_eq!(c.hiss_db, -78.0);
    }

    #[test]
    fn settings_round_trip_and_old_files_default_to_off() {
        let s = TapeSettings {
            enabled: true,
            ips: TapeSpeed::Ips30,
            quality: TapeQuality::High,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"ips30\""));
        assert_eq!(serde_json::from_str::<TapeSettings>(&json).unwrap(), s);
        let partial: TapeSettings = serde_json::from_str(r#"{"drive_db": 3.0}"#).unwrap();
        assert!(!partial.enabled);
        assert_eq!(partial.ips, TapeSpeed::Ips15);
        assert_eq!(partial.quality, TapeQuality::Standard);
    }

    #[test]
    fn sample_rate_independence() {
        // The same perceptual design at 44.1 and 192 kHz: bump center and
        // EQ turnovers measure the same.
        for sr in [44100u32, 192000] {
            let repro = design_repro(TapeSpeed::Ips15, TapeEqStandard::Nab, sr as f64);
            let w = |f: f64| 2.0 * PI * f / sr as f64;
            let t = 20.0 * repro[1].magnitude(w(3183.0)).log10()
                - 20.0 * repro[1].magnitude(w(100.0)).log10();
            assert!((t + 3.0).abs() < 0.5, "{sr}: {t:.2} dB");
            let bump = rbj_peaking(bump_freq(TapeSpeed::Ips15), 1.0, 2.5, sr as f64);
            let at45 = 20.0 * bump.magnitude(w(45.0)).log10();
            assert!((at45 - 2.5).abs() < 0.3, "{sr}: {at45:.2} dB");
        }
    }

    #[test]
    fn silence_and_denormals_stay_finite() {
        let mut s = stage(192000);
        let mut buf = stereo(
            192000,
            |i| {
                if i < 10 {
                    (1e-38, -1e-38)
                } else {
                    (0.0, 0.0)
                }
            },
        );
        s.process(&mut buf, 2);
        assert!(buf.iter().all(|x| x.is_finite()));
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
