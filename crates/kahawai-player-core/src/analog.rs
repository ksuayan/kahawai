//! Analog character: an optional tube or transistor "warmth" stage for the
//! shared PCM path. (Plan: Analog-Emulation.md, Phases 1 and 2.)
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

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::dsp::DspStage;

/// Which character to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnalogFlavour {
    /// A 12AX7 triode stage (Koren model): mostly 2nd harmonic (even).
    #[default]
    WarmTriode,
    /// Symmetric soft curve: odd harmonics only, like a transistor stage.
    SolidState,
}

/// How to keep aliasing out of the audible band. `Auto` follows the sample
/// rate (see [`anti_alias_plan`]); the others force a plan, for A/B listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AntiAliasChoice {
    #[default]
    Auto,
    X1,
    X1Adaa,
    X2,
    X2Adaa,
    X4,
    X4Adaa,
}

impl AntiAliasChoice {
    /// The plan this choice means at `sample_rate`.
    pub fn resolve(self, sample_rate: u32) -> AntiAlias {
        let (factor, adaa) = match self {
            Self::Auto => return anti_alias_plan(sample_rate),
            Self::X1 => (1, false),
            Self::X1Adaa => (1, true),
            Self::X2 => (2, false),
            Self::X2Adaa => (2, true),
            Self::X4 => (4, false),
            Self::X4Adaa => (4, true),
        };
        AntiAlias { factor, adaa }
    }

    fn from_plan(plan: AntiAlias) -> Self {
        match (plan.factor, plan.adaa) {
            (1, false) => Self::X1,
            (1, true) => Self::X1Adaa,
            (2, false) => Self::X2,
            (2, true) => Self::X2Adaa,
            (_, false) => Self::X4,
            (_, true) => Self::X4Adaa,
        }
    }
}

/// User-facing settings; persisted in `engine-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalogSettings {
    pub enabled: bool,
    pub flavour: AnalogFlavour,
    /// 0..=1: how hard the signal is pushed into the curve.
    pub drive: f32,
    /// 0..=1: parallel blend of the processed signal (1 = fully processed).
    pub mix: f32,
    /// Output trim in dB, -6..=6.
    pub output_db: f32,
    /// Match the processed level to the dry level (at a -12 dBFS reference).
    pub auto_gain: bool,
    /// Anti-aliasing plan; `auto` follows the sample rate.
    pub antialias: AntiAliasChoice,
}

impl Default for AnalogSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            flavour: AnalogFlavour::WarmTriode,
            drive: 0.4,
            mix: 0.4,
            output_db: 0.0,
            auto_gain: true,
            antialias: AntiAliasChoice::Auto,
        }
    }
}

impl AnalogSettings {
    /// Pull every value into its allowed range.
    pub fn clamped(mut self) -> Self {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        self.drive = fin(self.drive, 0.4).clamp(0.0, 1.0);
        self.mix = fin(self.mix, 0.4).clamp(0.0, 1.0);
        self.output_db = fin(self.output_db, 0.0).clamp(-6.0, 6.0);
        self
    }
}

// ---------------------------------------------------------------------------
// The tube model (Koren)
// ---------------------------------------------------------------------------

/// Koren's 12AX7 parameters (normankoren.com, "Improved vacuum tube models
/// for SPICE simulations"): mu, Ex, Kg1, Kp, Kvb.
const MU: f64 = 100.0;
const EX: f64 = 1.4;
const KG1: f64 = 1060.0;
const KP: f64 = 600.0;
const KVB: f64 = 300.0;

/// The stage around the tube: supply and plate load, and the grid bias the
/// cathode resistor would set.
const B_PLUS: f64 = 300.0;
const R_LOAD: f64 = 100_000.0;
const V_BIAS: f64 = -1.5;
/// Grid volts per unit of curve input.
const GRID_SWING: f64 = 1.5;
/// Positive-grid behaviour: the grid conducts (about 2 kΩ) against the
/// driving source (10 kΩ), so a positive grid swing is squashed hard.
const GRID_K: f64 = 2_000.0 / (2_000.0 + 10_000.0);
/// Softness of the grid-conduction corner, in volts.
const GRID_KNEE: f64 = 0.05;

/// Plate current (amps) of the Koren triode model.
pub fn koren_plate_current(vgk: f64, vpk: f64) -> f64 {
    let z = KP * (1.0 / MU + vgk / (KVB + vpk * vpk).sqrt());
    let softplus = if z > 30.0 { z } else { (1.0 + z.exp()).ln() };
    let e1 = vpk / KP * softplus;
    if e1 <= 0.0 { 0.0 } else { 2.0 * e1.powf(EX) / KG1 }
}

/// Plate voltage for a grid voltage, on the resistive load line
/// `vp = B+ - Ip(vg, vp) * RL` (bisection: the current falls as vp rises).
pub fn triode_plate_voltage(vgk: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, B_PLUS);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if mid - (B_PLUS - koren_plate_current(vgk, mid) * R_LOAD) > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Grid voltage for a curve input `u`: the bias plus the swing, with the
/// positive half squashed by grid conduction.
fn grid_voltage(u: f64) -> f64 {
    let v = V_BIAS + GRID_SWING * u;
    let sp = GRID_KNEE * (1.0 + (v / GRID_KNEE).min(40.0).exp()).ln();
    let sp = if v / GRID_KNEE > 40.0 { v } else { sp };
    v - (1.0 - GRID_K) * sp
}

/// The triode stage as a curve `f(u)`: input polarity kept, zero at zero,
/// unit slope at zero. Tabulated once, with its antiderivative, so the
/// audio path is a lookup (and first-order antiderivative antialiasing
/// is exact for the interpolated curve).
pub struct TubeTable {
    lo: f64,
    step: f64,
    f: Vec<f64>,
    /// `anti[i]` = integral of the interpolated curve from `lo` to node i.
    anti: Vec<f64>,
}

const TABLE_RANGE: f64 = 4.0; // curve input beyond +-4 is held at the end value
const TABLE_POINTS: usize = 4096;

impl TubeTable {
    fn build() -> Self {
        let step = 2.0 * TABLE_RANGE / TABLE_POINTS as f64;
        let lo = -TABLE_RANGE;
        // Inverted (plate voltage falls as the grid rises) so polarity is kept.
        let raw: Vec<f64> = (0..=TABLE_POINTS)
            .map(|i| -triode_plate_voltage(grid_voltage(lo + i as f64 * step)))
            .collect();
        let mid = TABLE_POINTS / 2;
        let slope = (raw[mid + 1] - raw[mid - 1]) / (2.0 * step);
        let f: Vec<f64> = raw.iter().map(|v| (v - raw[mid]) / slope).collect();
        let mut anti = vec![0.0; f.len()];
        for i in 1..f.len() {
            anti[i] = anti[i - 1] + 0.5 * step * (f[i - 1] + f[i]);
        }
        Self { lo, step, f, anti }
    }

    /// The curve at `u`.
    pub fn eval(&self, u: f64) -> f64 {
        let p = (u - self.lo) / self.step;
        let last = (self.f.len() - 1) as f64;
        if p <= 0.0 {
            return self.f[0];
        }
        if p >= last {
            return self.f[self.f.len() - 1];
        }
        let i = p as usize;
        let fr = p - i as f64;
        self.f[i] * (1.0 - fr) + self.f[i + 1] * fr
    }

    /// The integral of the curve up to `u` (held flat beyond the table).
    pub fn antiderivative(&self, u: f64) -> f64 {
        let p = (u - self.lo) / self.step;
        let last = (self.f.len() - 1) as f64;
        if p <= 0.0 {
            return self.f[0] * (u - self.lo);
        }
        if p >= last {
            return self.anti[self.f.len() - 1] + self.f[self.f.len() - 1] * (u - (self.lo + last * self.step));
        }
        let i = p as usize;
        let t = (u - (self.lo + i as f64 * self.step)) / 1.0;
        let f0 = self.f[i];
        let slope = (self.f[i + 1] - f0) / self.step;
        self.anti[i] + f0 * t + 0.5 * slope * t * t
    }
}

/// The shared 12AX7 table (built on first use).
pub fn triode_table() -> &'static TubeTable {
    static TABLE: OnceLock<TubeTable> = OnceLock::new();
    TABLE.get_or_init(TubeTable::build)
}

// ---------------------------------------------------------------------------
// The curves
// ---------------------------------------------------------------------------

/// Input gain for a drive setting: 1x at 0, 8x at 1 (squared for a gentle start).
fn drive_gain(drive: f32) -> f32 {
    1.0 + 7.0 * drive * drive
}

/// `ln cosh x`, stable for large |x|.
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (1.0 + (-2.0 * a).exp()).ln() - std::f64::consts::LN_2
}

/// A curve of the drive-scaled input `u`, with unit slope at zero, and its
/// antiderivative (for ADAA).
#[derive(Clone, Copy)]
struct Shaper {
    table: Option<&'static TubeTable>,
}

impl Shaper {
    fn new(flavour: AnalogFlavour) -> Self {
        let table = match flavour {
            AnalogFlavour::WarmTriode => Some(triode_table()),
            AnalogFlavour::SolidState => None,
        };
        Self { table }
    }

    #[inline]
    fn f(&self, u: f64) -> f64 {
        match self.table {
            Some(t) => t.eval(u),
            None => u.tanh(),
        }
    }

    #[inline]
    fn anti(&self, u: f64) -> f64 {
        match self.table {
            Some(t) => t.antiderivative(u),
            None => ln_cosh(u),
        }
    }

    /// The curve, alias-protected by first-order antiderivative
    /// antialiasing when `adaa` is set (state: previous input and its
    /// antiderivative).
    #[inline]
    fn apply(&self, adaa: bool, st: &mut (f64, f64), u: f32) -> f32 {
        let u = u as f64;
        if !adaa {
            return self.f(u) as f32;
        }
        let a = self.anti(u);
        let du = u - st.0;
        let y = if du.abs() < 1e-6 { self.f(0.5 * (u + st.0)) } else { (a - st.1) / du };
        *st = (u, a);
        y as f32
    }

    /// Initial ADAA state (input 0).
    fn rest(&self) -> (f64, f64) {
        (0.0, self.anti(0.0))
    }
}

/// Gain that makes the processed level match the dry level for a -12 dBFS
/// RMS sine: what "auto gain match" applies.
fn gain_match(shaper: Shaper, g: f32) -> f32 {
    const N: usize = 2048;
    let amp = 0.354_f64; // -12 dBFS RMS
    let mut sum_in = 0.0f64;
    let mut ys = [0.0f64; N];
    for (i, y) in ys.iter_mut().enumerate() {
        let x = amp * (2.0 * std::f64::consts::PI * (i as f64) / 64.0).sin();
        sum_in += x * x;
        *y = shaper.f(g as f64 * x) / g as f64;
    }
    let mean = ys.iter().sum::<f64>() / N as f64; // DC is removed downstream
    let sum_out: f64 = ys.iter().map(|&v| (v - mean).powi(2)).sum();
    if sum_out <= 1e-12 {
        return 1.0;
    }
    ((sum_in / sum_out).sqrt() as f32).clamp(0.25, 4.0)
}

// ---------------------------------------------------------------------------
// Anti-aliasing plan and the oversampling filter
// ---------------------------------------------------------------------------

/// FIR taps per polyphase branch. Latency is `TAPS_PER_PHASE` base-rate frames.
const TAPS_PER_PHASE: usize = 32;

/// How aliasing is kept out of the audible band for a sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntiAlias {
    /// Oversampling factor (1 = none).
    pub factor: usize,
    /// First-order antiderivative antialiasing on the curve.
    pub adaa: bool,
}

/// The plan for a sample rate, chosen from the measurements in
/// Analog-Emulation.md (section 12).
pub fn anti_alias_plan(sample_rate: u32) -> AntiAlias {
    match sample_rate {
        0..=50_000 => AntiAlias { factor: 4, adaa: true },
        50_001..=100_000 => AntiAlias { factor: 2, adaa: true },
        _ => AntiAlias { factor: 1, adaa: true },
    }
}

/// Oversampling factor for a sample rate.
pub fn oversample_factor(sample_rate: u32) -> usize {
    anti_alias_plan(sample_rate).factor
}

/// Kaiser-windowed low-pass, unity DC gain, cutoff as a fraction of the high rate.
fn kaiser_lowpass(taps: usize, cutoff: f64) -> Vec<f32> {
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
    let pi = std::f64::consts::PI;
    let h: Vec<f64> = (0..taps)
        .map(|n| {
            let k = n as f64 - m / 2.0;
            let sinc = if k == 0.0 { 2.0 * cutoff } else { (2.0 * pi * cutoff * k).sin() / (pi * k) };
            let w = i0(beta * (1.0 - (2.0 * n as f64 / m - 1.0).powi(2)).max(0.0).sqrt()) / i0(beta);
            sinc * w
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| (v / sum) as f32).collect()
}

/// A doubled ring buffer: the newest `len` values are always one slice.
struct Ring {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
}

impl Ring {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len * 2], len, pos: 0 }
    }
    fn push(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.buf[self.pos + self.len] = v;
        self.pos = (self.pos + 1) % self.len;
    }
    /// Value pushed `age` pushes ago (0 = newest).
    #[inline]
    fn at(&self, age: usize) -> f32 {
        self.buf[self.pos + self.len - 1 - age]
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
    }
}

/// Per-channel state.
struct Chan {
    /// Recent input samples (for the interpolator).
    xin: Ring,
    /// Recent shaped high-rate samples (for the decimator).
    yhr: Ring,
    /// Dry path delay, matching the oversampler's latency.
    dry: Ring,
    dc_x1: f32,
    dc_y1: f32,
    /// ADAA memory: previous curve input and its antiderivative.
    adaa: (f64, f64),
}

impl Chan {
    fn new(taps_phase: usize, taps_total: usize, latency: usize, shaper: &Shaper) -> Self {
        Self {
            xin: Ring::new(taps_phase),
            yhr: Ring::new(taps_total),
            dry: Ring::new(latency.max(1)),
            dc_x1: 0.0,
            dc_y1: 0.0,
            adaa: shaper.rest(),
        }
    }
    fn clear(&mut self, shaper: &Shaper) {
        self.xin.clear();
        self.yhr.clear();
        self.dry.clear();
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
        self.adaa = shaper.rest();
    }
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

/// How long parameter changes and on/off take to fade, in seconds.
const RAMP_SECONDS: f32 = 0.015;

/// What the stage is doing right now, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalogStatus {
    pub plan: AntiAlias,
    pub latency_frames: u32,
    pub sample_rate: u32,
}

impl AnalogStatus {
    /// e.g. "4x + ADAA, 0.7 ms latency".
    pub fn describe(&self) -> String {
        let ms = self.latency_frames as f64 * 1000.0 / self.sample_rate.max(1) as f64;
        let alias = match (self.plan.factor, self.plan.adaa) {
            (1, false) => "no anti-aliasing".to_string(),
            (1, true) => "ADAA".to_string(),
            (f, false) => format!("{f}x oversampling"),
            (f, true) => format!("{f}x oversampling + ADAA"),
        };
        format!("{alias}, {ms:.1} ms latency")
    }
}

pub struct AnalogStage {
    settings: AnalogSettings,
    plan: AntiAlias,
    shaper: Shaper,
    /// The curve in use. A flavour change waits for a fade-out (see `pending`).
    active: AnalogFlavour,
    pending: Option<AnalogFlavour>,
    /// A changed anti-aliasing plan also waits for a fade-out.
    pending_plan: Option<AntiAlias>,
    sample_rate: u32,
    l: usize,
    h: Vec<f32>,
    chans: Vec<Chan>,
    // Smoothed values, gliding toward the targets.
    g: f32,
    comp: f32,
    out: f32,
    mix: f32,
    /// 1 when the stage is on, 0 when off; fades between.
    fade: f32,
    primed: bool,
    dc_r: f32,
}

impl AnalogStage {
    pub fn new(sample_rate: u32) -> Self {
        Self::with_plan(sample_rate, anti_alias_plan(sample_rate))
    }

    /// A stage with an explicit anti-aliasing plan (for measurements).
    pub fn with_plan(sample_rate: u32, plan: AntiAlias) -> Self {
        let mut s = Self {
            settings: AnalogSettings { antialias: AntiAliasChoice::from_plan(plan), ..AnalogSettings::default() },
            plan,
            shaper: Shaper::new(AnalogFlavour::default()),
            active: AnalogFlavour::default(),
            pending: None,
            pending_plan: None,
            sample_rate,
            l: 1,
            h: Vec::new(),
            chans: Vec::new(),
            g: 1.0,
            comp: 1.0,
            out: 1.0,
            mix: 0.0,
            fade: 0.0,
            primed: false,
            dc_r: 0.999,
        };
        s.design();
        s.snap();
        s
    }

    fn design(&mut self) {
        self.l = self.plan.factor.max(1);
        self.h = if self.l > 1 {
            kaiser_lowpass(TAPS_PER_PHASE * self.l + 1, 0.45 / self.l as f64)
        } else {
            Vec::new()
        };
        self.dc_r = 1.0 - 2.0 * std::f32::consts::PI * 10.0 / self.sample_rate.max(1) as f32;
        self.chans.clear();
    }

    fn targets(&self) -> (f32, f32, f32, f32) {
        let s = &self.settings;
        let g = drive_gain(s.drive);
        let comp = if s.auto_gain { gain_match(self.shaper, g) } else { 1.0 };
        let out = 10f32.powf(s.output_db / 20.0);
        (g, comp, out, s.mix)
    }

    fn snap_params(&mut self) {
        let (g, comp, out, mix) = self.targets();
        self.g = g;
        self.comp = comp;
        self.out = out;
        self.mix = mix;
    }

    fn snap(&mut self) {
        self.snap_params();
        self.fade = if self.settings.enabled { 1.0 } else { 0.0 };
    }

    pub fn settings(&self) -> AnalogSettings {
        self.settings
    }

    /// Apply new settings. While audio is flowing, changes glide in (about
    /// 15 ms), and a change of flavour fades out, swaps and fades back in.
    /// Before any audio has passed, values apply at once (the on/off fade
    /// always runs, so switching on from bypass is never a jump).
    pub fn set_settings(&mut self, settings: AnalogSettings) {
        self.settings = settings.clamped();
        let want = self.settings.flavour;
        let idle = !self.primed || self.fade == 0.0;
        if want == self.active {
            self.pending = None;
        } else if idle {
            self.active = want;
            self.shaper = Shaper::new(want);
            self.pending = None;
        } else {
            self.pending = Some(want);
        }
        let want_plan = self.settings.antialias.resolve(self.sample_rate);
        if want_plan == self.plan {
            self.pending_plan = None;
        } else if idle {
            self.plan = want_plan;
            self.pending_plan = None;
            self.design();
        } else {
            self.pending_plan = Some(want_plan);
        }
        if !self.primed {
            self.snap_params();
        }
    }

    /// The plan in use and its latency, while the stage is on.
    pub fn status(&self) -> Option<AnalogStatus> {
        (self.settings.enabled || self.fade > 0.0).then(|| AnalogStatus {
            plan: self.plan,
            latency_frames: self.latency() as u32,
            sample_rate: self.sample_rate,
        })
    }

    /// Apply a waiting flavour or plan change (the caller has faded out).
    fn swap_pending(&mut self) {
        if let Some(f) = self.pending.take() {
            self.active = f;
            self.shaper = Shaper::new(f);
        }
        if let Some(p) = self.pending_plan.take() {
            if p != self.plan {
                self.plan = p;
                self.design();
            }
        }
    }

    /// Latency added while the stage is on, in frames at the current rate.
    fn latency(&self) -> usize {
        if self.l > 1 { TAPS_PER_PHASE } else { 0 }
    }

    fn ensure_chans(&mut self, channels: usize) {
        if self.chans.len() != channels {
            // The decimator reads up to (L - 1) + (taps - 1) samples back.
            let taps_total = (self.h.len() + self.l).max(1);
            let (lat, shaper) = (self.latency(), self.shaper);
            self.chans = (0..channels)
                .map(|_| Chan::new(TAPS_PER_PHASE + 1, taps_total, lat, &shaper))
                .collect();
        }
    }
}

impl DspStage for AnalogStage {
    fn prepare(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.plan = self.settings.antialias.resolve(sample_rate);
            self.pending_plan = None;
            self.design();
            self.primed = false;
            self.snap();
        }
    }

    fn latency_frames(&self) -> u32 {
        if self.settings.enabled || self.fade > 0.0 { self.latency() as u32 } else { 0 }
    }

    fn reset(&mut self) {
        let shaper = self.shaper;
        self.chans.iter_mut().for_each(|c| c.clear(&shaper));
        self.primed = false;
        self.snap();
    }

    fn process(&mut self, samples: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        if self.fade == 0.0 {
            self.swap_pending();
        }
        let waiting = self.pending.is_some() || self.pending_plan.is_some();
        let mut target_fade = if self.settings.enabled && !waiting { 1.0 } else { 0.0 };
        if self.fade == 0.0 && target_fade == 0.0 {
            return; // bit-transparent bypass
        }
        self.ensure_chans(channels);
        if self.fade == 0.0 {
            // Coming back from bypass: stale filter history would ring.
            let shaper = self.shaper;
            self.chans.iter_mut().for_each(|c| c.clear(&shaper));
        }
        self.primed = true;

        let (tg, mut tcomp, tout, tmix) = self.targets();
        let ramp = ((self.sample_rate as f32 * RAMP_SECONDS) as usize).max(1) as f32;
        let (mut sg, mut sc, mut so, mut sm, mut sf) = (
            (tg - self.g) / ramp,
            (tcomp - self.comp) / ramp,
            (tout - self.out) / ramp,
            (tmix - self.mix) / ramp,
            (target_fade - self.fade) / ramp,
        );
        let mut l = self.l;
        let mut adaa = self.plan.adaa;
        let mut latency = self.latency();
        let (dc_r, taps_phase) = (self.dc_r, TAPS_PER_PHASE + 1);
        let mut gain_up = l as f32;

        for frame in samples.chunks_exact_mut(channels) {
            // Glide the parameters (linear, per frame).
            step(&mut self.g, tg, &mut sg);
            step(&mut self.comp, tcomp, &mut sc);
            step(&mut self.out, tout, &mut so);
            step(&mut self.mix, tmix, &mut sm);
            step(&mut self.fade, target_fade, &mut sf);
            if self.fade == 0.0 && (self.pending.is_some() || self.pending_plan.is_some()) {
                // Faded out mid-block: swap the curve and plan, clear their
                // history and fade back in, right here (not at the next block).
                self.swap_pending();
                self.ensure_chans(channels);
                l = self.l;
                adaa = self.plan.adaa;
                latency = self.latency();
                gain_up = l as f32;
                let shaper = self.shaper;
                self.chans.iter_mut().for_each(|c| c.clear(&shaper));
                self.comp = if self.settings.auto_gain { gain_match(shaper, tg) } else { 1.0 };
                tcomp = self.comp;
                sc = 0.0;
                target_fade = if self.settings.enabled { 1.0 } else { 0.0 };
                sf = (target_fade - self.fade) / ramp;
            }
            let (comp, out, mix, fade) = (self.comp, self.out, self.mix, self.fade);
            let shaper = self.shaper;
            let (gd, inv_g) = (self.g, 1.0 / self.g);

            for (ch, smp) in frame.iter_mut().enumerate() {
                let x = *smp;
                let st = &mut self.chans[ch];
                let wet = if l == 1 {
                    shaper.apply(adaa, &mut st.adaa, x * gd) * inv_g
                } else {
                    // Interpolate: L high-rate samples per input, shaped as they are made.
                    st.xin.push(x);
                    for p in 0..l {
                        let mut acc = 0.0f32;
                        for k in 0..taps_phase {
                            let idx = p + k * l;
                            if idx < self.h.len() {
                                acc += self.h[idx] * st.xin.at(k);
                            }
                        }
                        st.yhr.push(shaper.apply(adaa, &mut st.adaa, acc * gain_up * gd) * inv_g);
                    }
                    // Decimate: one output per input, from the sample L-1 pushes back.
                    let mut acc = 0.0f32;
                    for (j, hv) in self.h.iter().enumerate() {
                        acc += hv * st.yhr.at(l - 1 + j);
                    }
                    acc
                };
                // Remove the DC an asymmetric curve creates.
                let dc = wet - st.dc_x1 + dc_r * st.dc_y1;
                st.dc_x1 = wet;
                st.dc_y1 = dc;
                let wet = dc * comp * out;

                // Dry path, delayed to line up with the oversampled wet path.
                let dry = if latency > 0 {
                    let d = st.dry.at(latency - 1);
                    st.dry.push(x);
                    d
                } else {
                    x
                };
                let m = mix * fade;
                let processed = dry + (wet - dry) * m;
                // The off state is the un-delayed input: fade between them.
                *smp = x + (processed - x) * fade;
            }
        }
        if self.fade == 0.0 {
            // Fully faded out: the next call is a bit-transparent bypass.
            self.primed = false;
        }
    }
}

/// Move `cur` toward `target` by `step` per call, landing exactly.
#[inline]
fn step(cur: &mut f32, target: f32, step: &mut f32) {
    if *cur == target {
        return;
    }
    let next = *cur + *step;
    *cur = if (*step >= 0.0 && next >= target) || (*step < 0.0 && next <= target) { target } else { next };
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{EqBand, EqBandType, ParametricEq};
    use std::f64::consts::PI;

    fn on(flavour: AnalogFlavour, drive: f32, mix: f32) -> AnalogSettings {
        AnalogSettings { enabled: true, flavour, drive, mix, output_db: 0.0, auto_gain: false, antialias: AntiAliasChoice::Auto }
    }

    fn tone(bin: usize, n: usize, periods: usize, amp: f32, channels: usize) -> Vec<f32> {
        (0..n * periods)
            .flat_map(|i| {
                let v = amp * (2.0 * PI * bin as f64 * i as f64 / n as f64).sin() as f32;
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    fn mono(x: &[f32], channels: usize) -> Vec<f64> {
        x.iter().step_by(channels).map(|&v| v as f64).collect()
    }

    fn spectrum(x: &[f64]) -> Vec<f64> {
        let n = x.len();
        let (mut re, mut im) = (x.to_vec(), vec![0.0; n]);
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j ^= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let ang = -2.0 * PI / len as f64;
            for i in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                    let (a, b) = (i + k, i + k + len / 2);
                    let (vr, vi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    re[b] = re[a] - vr;
                    im[b] = im[a] - vi;
                    re[a] += vr;
                    im[a] += vi;
                }
            }
            len <<= 1;
        }
        (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * 2.0 / n as f64).collect()
    }

    fn db(x: f64) -> f64 {
        20.0 * x.max(1e-12).log10()
    }

    /// Run a stereo tone through the stage, return the 3rd of 4 periods
    /// (settled, with samples on both sides).
    fn run(stage: &mut AnalogStage, bin: usize, n: usize, amp: f32) -> Vec<f64> {
        let mut x = tone(bin, n, 4, amp, 2);
        stage.process(&mut x, 2);
        mono(&x, 2)[2 * n..3 * n].to_vec()
    }

    fn max_step(x: &[f32], channels: usize) -> f32 {
        let m: Vec<f32> = x.iter().step_by(channels).copied().collect();
        m.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn disabled_stage_is_bit_transparent() {
        let mut st = AnalogStage::new(44_100);
        let input = tone(200, 4096, 1, 0.5, 2);
        let mut out = input.clone();
        st.process(&mut out, 2);
        assert_eq!(out, input, "off from the start: untouched");

        // On, then off again: once faded out, bit-transparent once more.
        st.set_settings(on(AnalogFlavour::WarmTriode, 0.5, 1.0));
        let mut warm = tone(200, 4096, 2, 0.5, 2);
        st.process(&mut warm, 2);
        st.set_settings(AnalogSettings { enabled: false, ..on(AnalogFlavour::WarmTriode, 0.5, 1.0) });
        let mut fading = tone(200, 4096, 2, 0.5, 2);
        st.process(&mut fading, 2);
        let mut after = input.clone();
        st.process(&mut after, 2);
        assert_eq!(after, input, "faded out: untouched again");
    }

    #[test]
    fn warm_triode_gives_even_harmonics_and_solid_state_odd() {
        let (n, bin) = (1 << 14, 200);
        let mut warm = AnalogStage::new(44_100);
        warm.set_settings(on(AnalogFlavour::WarmTriode, 0.4, 1.0));
        let sp = spectrum(&run(&mut warm, bin, n, 0.2));
        let (h2, h3) = (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]));
        assert!(h2 > h3 + 10.0, "triode: 2nd ({h2:.1} dB) well above 3rd ({h3:.1} dB)");
        assert!(h2 > -30.0 && h2 < -6.0, "audible but not extreme 2nd harmonic: {h2:.1} dB");

        let mut solid = AnalogStage::new(44_100);
        solid.set_settings(on(AnalogFlavour::SolidState, 0.6, 1.0));
        let sp = spectrum(&run(&mut solid, bin, n, 0.5));
        let (h2, h3) = (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]));
        assert!(h2 < -70.0, "solid state: no even harmonics, 2nd at {h2:.1} dB");
        assert!(h3 > -50.0 && h3 < -6.0, "solid state: audible 3rd harmonic {h3:.1} dB");
    }

    #[test]
    fn harmonics_grow_with_level_and_drive() {
        let (n, bin) = (1 << 14, 200);
        let h2 = |drive: f32, amp: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(on(AnalogFlavour::WarmTriode, drive, 1.0));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            db(sp[bin * 2] / sp[bin])
        };
        assert!(h2(0.5, 0.4) > h2(0.5, 0.1) + 6.0, "louder input: more harmonic");
        assert!(h2(0.7, 0.1) > h2(0.4, 0.1) + 3.0, "more drive: more harmonic");
        assert!(h2(0.0, 0.05) < -40.0, "a quiet signal at zero drive stays very clean");
    }

    /// Everything audible that is not a harmonic of the tone is aliasing.
    fn alias_db_with(fs: u32, tone_hz: f64, drive: f32, amp: f32, flavour: AnalogFlavour, plan: Option<AntiAlias>) -> f64 {
        let n = 1 << 14;
        let bin = (tone_hz / fs as f64 * n as f64).round() as usize;
        let mut st = match plan {
            Some(p) => AnalogStage::with_plan(fs, p),
            None => AnalogStage::new(fs),
        };
        let aa = plan.map(AntiAliasChoice::from_plan).unwrap_or_default();
        st.set_settings(AnalogSettings { antialias: aa, ..on(flavour, drive, 1.0) });
        let sp = spectrum(&run(&mut st, bin, n, amp));
        let lim = (20_000.0 / fs as f64 * n as f64) as usize;
        let err = (1..lim.min(n / 2))
            .filter(|k| {
                let r = k % bin;
                r > 2 && r < bin - 2
            })
            .map(|k| sp[k] * sp[k])
            .sum::<f64>()
            .sqrt();
        db(err / sp[bin])
    }

    fn alias_db(fs: u32, tone_hz: f64, drive: f32, amp: f32) -> f64 {
        alias_db_with(fs, tone_hz, drive, amp, AnalogFlavour::WarmTriode, None)
    }

    /// Prints the aliasing of every plan (run with --ignored --nocapture); the
    /// table in Analog-Emulation.md section 12 comes from this.
    #[test]
    #[ignore]
    fn measure_anti_alias_plans() {
        let plans: [(usize, bool); 6] = [(1, false), (1, true), (2, false), (2, true), (4, false), (4, true)];
        for (flavour, name) in [(AnalogFlavour::WarmTriode, "warm triode"), (AnalogFlavour::SolidState, "solid state")] {
            for (label, drive, amp, tone) in [("hard test tone (drive 0.53, in 0.8, 9.5 kHz)", 0.53f32, 0.8f32, 9_500.0), ("typical (drive 0.4, in 0.3, 5 kHz)", 0.4, 0.3, 5_000.0)] {
                for fs in [44_100u32, 96_000] {
                    let row: Vec<String> = plans
                        .iter()
                        .map(|&(f, a)| format!("{}x{}: {:>6.1}", f, if a { "+ADAA" } else { "     " }, alias_db_with(fs, tone, drive, amp, flavour, Some(AntiAlias { factor: f, adaa: a }))))
                        .collect();
                    println!("{name:<12} {label:<44} {fs:>6} Hz | {}", row.join(" | "));
                }
            }
        }
    }

    #[test]
    fn oversampling_keeps_aliasing_below_audibility() {
        // A hard test tone (research/analog-spike: the same drive without
        // protection aliases at -14 dB at 44.1 kHz).
        let a441 = alias_db(44_100, 9_500.0, 0.53, 0.8);
        assert!(a441 < -65.0, "44.1 kHz aliasing {a441:.1} dB");
        let a48 = alias_db(48_000, 9_500.0, 0.53, 0.8);
        assert!(a48 < -65.0, "48 kHz aliasing {a48:.1} dB");
        let a96 = alias_db(96_000, 9_500.0, 0.53, 0.8);
        assert!(a96 < -70.0, "96 kHz aliasing {a96:.1} dB");
        let a192 = alias_db(192_000, 9_500.0, 0.53, 0.8);
        assert!(a192 < -70.0, "192 kHz aliasing {a192:.1} dB (ADAA only, no oversampling)");
        assert_eq!((oversample_factor(44_100), oversample_factor(96_000), oversample_factor(192_000)), (4, 2, 1));
    }

    #[test]
    fn koren_model_gives_plausible_12ax7_currents() {
        // Datasheet-scale checks (RCA bogey: about 1.2 mA at 250 V, -2 V grid).
        let ip = koren_plate_current(-2.0, 250.0) * 1000.0;
        assert!((0.7..1.6).contains(&ip), "Ip(-2 V, 250 V) = {ip:.2} mA");
        assert!(koren_plate_current(-6.0, 250.0) < 1e-5, "cut off well below the bias");
        let (a, b, c) = (koren_plate_current(-3.0, 250.0), koren_plate_current(-2.0, 250.0), koren_plate_current(-1.0, 250.0));
        assert!(a < b && b < c, "more current as the grid rises");
        let (p1, p2) = (triode_plate_voltage(-3.0), triode_plate_voltage(-0.5));
        assert!(p1 > p2 && p1 < B_PLUS && p2 > 0.0, "plate voltage falls as the grid rises: {p1:.0} V -> {p2:.0} V");
    }

    #[test]
    fn tube_table_is_a_unit_slope_asymmetric_curve() {
        let t = triode_table();
        assert!(t.eval(0.0).abs() < 1e-9, "zero at zero");
        let slope = (t.eval(0.001) - t.eval(-0.001)) / 0.002;
        assert!((slope - 1.0).abs() < 0.01, "unit small-signal slope, got {slope}");
        let mut prev = f64::MIN;
        for i in -400..=400 {
            let v = t.eval(i as f64 * 0.01);
            assert!(v >= prev - 1e-12, "non-decreasing at {}", i as f64 * 0.01);
            prev = v;
        }
        // Asymmetric: the two halves compress differently.
        let (up, down) = (t.eval(2.0), -t.eval(-2.0));
        assert!((up - down).abs() / up.max(down) > 0.15, "lopsided: +2 -> {up:.2}, -2 -> {down:.2}");
        assert!(t.eval(9.0) == t.eval(4.0) && t.eval(-9.0) == t.eval(-4.0), "held flat beyond the table");
        // The table agrees with the model it was built from.
        for u in [-3.7, -1.23, -0.4, 0.05, 0.9, 2.2, 3.9] {
            let direct = -triode_plate_voltage(grid_voltage(u));
            let mid = -triode_plate_voltage(grid_voltage(0.0));
            let raw_slope = {
                let h = 1e-3;
                (-triode_plate_voltage(grid_voltage(h)) + triode_plate_voltage(grid_voltage(-h))) / (2.0 * h)
            };
            let want = (direct - mid) / raw_slope;
            assert!((t.eval(u) - want).abs() < 2e-3 * want.abs().max(1.0), "table vs model at {u}");
        }
    }

    #[test]
    fn antiderivative_matches_the_curve() {
        let t = triode_table();
        for u in [-5.0, -3.99, -2.0, -0.31, 0.0, 0.77, 1.9, 3.5, 4.5] {
            let h = 1e-4;
            let numeric = (t.antiderivative(u + h) - t.antiderivative(u - h)) / (2.0 * h);
            assert!((numeric - t.eval(u)).abs() < 1e-4, "dF/du = f at {u}: {numeric} vs {}", t.eval(u));
        }
        // Continuous across a node and across the table edge.
        let e = TABLE_RANGE;
        assert!((t.antiderivative(e + 1e-9) - t.antiderivative(e - 1e-9)).abs() < 1e-6);
        // Analytic tanh antiderivative used by the solid-state curve.
        let shaper = Shaper::new(AnalogFlavour::SolidState);
        for u in [-3.0f64, -0.5, 0.0, 1.2, 6.0] {
            let h = 1e-5;
            assert!(((shaper.anti(u + h) - shaper.anti(u - h)) / (2.0 * h) - u.tanh()).abs() < 1e-6);
        }
    }

    #[test]
    fn adaa_reproduces_the_curve_for_slow_signals_and_constants() {
        for flavour in [AnalogFlavour::WarmTriode, AnalogFlavour::SolidState] {
            let sh = Shaper::new(flavour);
            let mut st = sh.rest();
            // A constant input: ADAA must equal f(c) once the state has settled.
            let y = (0..4).map(|_| sh.apply(true, &mut st, 0.8)).last().unwrap();
            assert!((y as f64 - sh.f(0.8)).abs() < 1e-6, "{flavour:?}: constant input");
            // A slowly moving input (a 50 Hz sine at 48 kHz). ADAA of order one
            // returns the curve at the midpoint of the last step (half a sample
            // of delay), so compare with that.
            let mut st = sh.rest();
            let u_at = |i: usize| 1.5 * (2.0 * PI * 50.0 * i as f64 / 48_000.0).sin();
            let worst = (1..2000)
                .map(|i| {
                    let y = sh.apply(true, &mut st, u_at(i) as f32) as f64;
                    (y - sh.f(0.5 * (u_at(i) + u_at(i - 1)))).abs()
                })
                .skip(2)
                .fold(0.0, f64::max);
            assert!(worst < 1e-4, "{flavour:?}: slow signal differs from f by {worst}");
        }
    }

    #[test]
    fn triode_stage_matches_the_prototype_harmonic_profile() {
        // Research prototype (Analog-Emulation.md 10.2): a 12AX7 stage at input
        // 0.3 gave 2nd -32.5 dB and 3rd -60 dB; the 2nd rises 1 dB per dB.
        let (n, bin) = (1 << 14, 200);
        let h = |amp: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(on(AnalogFlavour::WarmTriode, 0.0, 1.0));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
        };
        let (h2, h3) = h(0.3);
        assert!((-35.0..-30.0).contains(&h2), "2nd at input 0.3: {h2:.1} dB (prototype -32.5)");
        assert!(h3 < -52.0, "3rd at input 0.3: {h3:.1} dB (prototype -60)");
        let (h2_low, _) = h(0.15);
        assert!((h2 - h2_low - 6.0).abs() < 1.5, "2nd rises about 1 dB per dB of input");
        let (h2_hi, h3_hi) = h(1.0);
        assert!(h2_hi > h3_hi + 12.0, "even harmonics keep dominating at full swing ({h2_hi:.1} vs {h3_hi:.1})");
    }

    #[test]
    fn dry_and_wet_are_time_aligned_and_latency_is_reported() {
        let mut st = AnalogStage::new(44_100);
        assert_eq!(st.latency_frames(), 0, "no latency while off");
        st.set_settings(on(AnalogFlavour::SolidState, 0.0, 0.5));
        assert_eq!(st.latency_frames(), 32);
        // Drive 0 and a tiny signal: the wet path is linear, so any mix of the
        // two paths must equal the input delayed by the latency, if aligned.
        let (n, bin) = (1 << 13, 186);
        let input = tone(bin, n, 4, 0.001, 1);
        let mut out = input.clone();
        st.process(&mut out, 1);
        let start = 2 * n;
        let err: f32 = (start..start + n).map(|i| (out[i] - input[i - 32]).abs()).fold(0.0, f32::max);
        assert!(err < 0.001 * 0.06, "aligned: worst error {err} on a 0.001 tone");
        assert_eq!(AnalogStage::new(192_000).latency(), 0, "no oversampling, no latency at 192 kHz");
    }

    #[test]
    fn auto_gain_matches_the_processed_level_to_the_dry_level() {
        for flavour in [AnalogFlavour::WarmTriode, AnalogFlavour::SolidState] {
            for drive in [0.4, 1.0] {
                let mut st = AnalogStage::new(44_100);
                st.set_settings(AnalogSettings { auto_gain: true, ..on(flavour, drive, 1.0) });
                let (n, bin) = (1 << 14, 200);
                let out = run(&mut st, bin, n, 0.354);
                let rms = (out.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
                let want = 0.354 / 2f64.sqrt();
                assert!(
                    (db(rms / want)).abs() < 1.0,
                    "{flavour:?} drive {drive}: {:.2} dB off the dry level",
                    db(rms / want)
                );
            }
        }
    }

    #[test]
    fn live_changes_do_not_click() {
        let (n, bin) = (1 << 13, 40); // ~215 Hz
        let mut st = AnalogStage::new(44_100);
        st.set_settings(on(AnalogFlavour::WarmTriode, 0.2, 0.5));
        let mut a = tone(bin, n, 2, 0.4, 1);
        st.process(&mut a, 1);
        // Drive, mix and output all change at once, mid-signal.
        st.set_settings(AnalogSettings { output_db: 3.0, ..on(AnalogFlavour::WarmTriode, 0.9, 1.0) });
        let mut b = tone(bin, n, 2, 0.4, 1);
        let all = [a.clone(), b.clone()].concat();
        st.process(&mut b, 1);
        let seam = [a, b].concat();
        let _ = all;
        // A 215 Hz tone at 0.4 moves at most ~0.02 per sample (a little more with +3 dB).
        assert!(max_step(&seam[n..], 1) < 0.05, "no click on a live parameter change");
    }

    #[test]
    fn switching_flavour_live_fades_instead_of_jumping() {
        let (n, bin) = (1 << 13, 40);
        let mut st = AnalogStage::new(44_100);
        st.set_settings(on(AnalogFlavour::WarmTriode, 0.7, 1.0));
        let mut a = tone(bin, n, 2, 0.4, 1);
        st.process(&mut a, 1);
        st.set_settings(on(AnalogFlavour::SolidState, 0.7, 1.0));
        assert_eq!(st.active, AnalogFlavour::WarmTriode, "the swap waits for the fade-out");
        let mut b = tone(bin, n, 2, 0.4, 1);
        st.process(&mut b, 1);
        assert_eq!(st.active, AnalogFlavour::SolidState, "swapped once faded out");
        let seam = [a, b.clone()].concat();
        assert!(max_step(&seam[n..], 1) < 0.05, "no click across the flavour change");
        // ...and the new curve is really in use (no even harmonics any more).
        let sp = spectrum(&mono(&b[n..2 * n], 1));
        assert!(db(sp[bin * 2] / sp[bin]) < -60.0);
    }

    #[test]
    fn settings_are_clamped_and_defaults_fill_gaps() {
        let s = AnalogSettings { drive: 5.0, mix: -1.0, output_db: 40.0, ..Default::default() }.clamped();
        assert_eq!((s.drive, s.mix, s.output_db), (1.0, 0.0, 6.0));
        assert_eq!(AnalogSettings { drive: f32::NAN, ..Default::default() }.clamped().drive, 0.4);
        let parsed: AnalogSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(parsed.enabled && parsed.flavour == AnalogFlavour::WarmTriode && parsed.mix == 0.4);
        let json = serde_json::to_string(&AnalogSettings::default()).unwrap();
        assert!(json.contains("\"warm_triode\""), "{json}");
    }

    fn with_aa(flavour: AnalogFlavour, aa: AntiAliasChoice) -> AnalogSettings {
        AnalogSettings { antialias: aa, ..on(flavour, 0.7, 1.0) }
    }

    #[test]
    fn anti_alias_choices_map_to_plans_and_serialize_by_name() {
        assert_eq!(AntiAliasChoice::Auto.resolve(44_100), AntiAlias { factor: 4, adaa: true });
        assert_eq!(AntiAliasChoice::Auto.resolve(192_000), AntiAlias { factor: 1, adaa: true });
        assert_eq!(AntiAliasChoice::X2.resolve(44_100), AntiAlias { factor: 2, adaa: false });
        assert_eq!(AntiAliasChoice::X4Adaa.resolve(192_000), AntiAlias { factor: 4, adaa: true });
        for (c, name) in [(AntiAliasChoice::Auto, "auto"), (AntiAliasChoice::X1, "x1"), (AntiAliasChoice::X1Adaa, "x1_adaa"), (AntiAliasChoice::X2Adaa, "x2_adaa"), (AntiAliasChoice::X4, "x4")] {
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{name}\""));
        }
        let plan = AntiAlias { factor: 2, adaa: true };
        assert_eq!(AntiAliasChoice::from_plan(plan).resolve(48_000), plan);
    }

    #[test]
    fn a_forced_plan_is_used_and_reported() {
        let mut st = AnalogStage::new(44_100);
        st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X1));
        let s = st.status().expect("on");
        assert_eq!((s.plan.factor, s.plan.adaa, s.latency_frames), (1, false, 0));
        assert_eq!(s.describe(), "no anti-aliasing, 0.0 ms latency");
        st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X4Adaa));
        assert_eq!(st.status().unwrap().describe(), "4x oversampling + ADAA, 0.7 ms latency");
        // A forced plan survives a sample-rate change; auto follows it.
        st.prepare(96_000);
        assert_eq!(st.status().unwrap().plan, AntiAlias { factor: 4, adaa: true });
        st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::Auto));
        assert_eq!(st.status().unwrap().plan, AntiAlias { factor: 2, adaa: true });
        assert!(AnalogStage::new(44_100).status().is_none(), "off: nothing to report");
    }

    #[test]
    fn switching_plans_live_fades_without_a_click() {
        let (n, bin) = (1 << 13, 40);
        let mut st = AnalogStage::new(44_100);
        st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X4Adaa));
        let mut a = tone(bin, n, 2, 0.4, 1);
        st.process(&mut a, 1);
        st.set_settings(with_aa(AnalogFlavour::WarmTriode, AntiAliasChoice::X1));
        assert_eq!(st.status().unwrap().plan.factor, 4, "the swap waits for the fade-out");
        let mut b = tone(bin, n, 2, 0.4, 1);
        st.process(&mut b, 1);
        let s = st.status().unwrap();
        assert_eq!((s.plan.factor, s.latency_frames), (1, 0), "swapped once faded out");
        let seam = [a, b.clone()].concat();
        assert!(max_step(&seam[n..], 1) < 0.05, "no click across the plan change");
        // ...and it came back at full level, not stuck faded out.
        let rms = (b[n..].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
        assert!(rms > 0.2, "signal present after the swap: rms {rms}");
    }

    #[test]
    fn stages_share_one_trait() {
        let mut eq = ParametricEq::new(44_100);
        eq.set_bands(vec![EqBand { band_type: EqBandType::Peaking, freq: 1000.0, gain_db: 6.0, q: 1.0 }]).unwrap();
        let mut stages: Vec<Box<dyn DspStage>> = vec![Box::new(eq), Box::new(AnalogStage::new(44_100))];
        let input = tone(200, 4096, 1, 0.3, 2);
        let mut out = input.clone();
        for s in stages.iter_mut() {
            s.prepare(44_100);
            s.process(&mut out, 2);
        }
        assert_ne!(out, input, "the EQ stage changed the audio");
        assert_eq!(stages[0].latency_frames(), 0);
        stages.iter_mut().for_each(|s| s.reset());
    }
}
