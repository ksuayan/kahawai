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
    /// Class-A push-pull pair of 2A3 power triodes: odd harmonics, a little
    /// even from imperfect matching.
    PushPull,
    /// Symmetric soft curve: odd harmonics only, like a transistor stage.
    SolidState,
    /// A near-hard symmetric clip: clean below the knee, harsh above it.
    HardTransistor,
    /// 12AT7 / ECC81: medium-high mu small-signal triode.
    #[serde(rename = "tube_12at7")]
    Tube12at7,
    /// 12AU7 / ECC82: low-mu, clean small-signal triode.
    #[serde(rename = "tube_12au7")]
    Tube12au7,
    /// 6SN7: low-mu octal triode.
    #[serde(rename = "tube_6sn7")]
    Tube6sn7,
    /// 6DJ8 / ECC88: medium-mu low-noise triode.
    #[serde(rename = "tube_6dj8")]
    Tube6dj8,
    /// 300B: single-ended directly-heated power triode.
    #[serde(rename = "tube_300b")]
    Tube300b,
    /// 2A3: single-ended directly-heated power triode.
    #[serde(rename = "tube_2a3")]
    Tube2a3,
    /// 6SL7GT: high-mu octal triode.
    #[serde(rename = "tube_6sl7")]
    Tube6sl7,
    /// 12AY7: low-noise medium-mu triode.
    #[serde(rename = "tube_12ay7")]
    Tube12ay7,
    /// 12AX7A (Sylvania fit): a second 12AX7 flavour.
    #[serde(rename = "tube_12ax7a")]
    Tube12ax7a,
    /// EL84: single-ended class-A pentode.
    #[serde(rename = "tube_el84")]
    TubeEl84,
    /// EL34 pair, push-pull class AB.
    #[serde(rename = "push_pull_el34")]
    PushPullEl34,
    /// 6L6GC pair, push-pull class AB.
    #[serde(rename = "push_pull_6l6gc")]
    PushPull6l6gc,
    /// KT88 pair, push-pull class AB.
    #[serde(rename = "push_pull_kt88")]
    PushPullKt88,
    /// A JFET stage: square-law, 2nd-harmonic warmth.
    Jfet,
    /// A silicon diode-pair clipper: symmetric soft clip.
    SiliconDiode,
    /// Germanium against silicon diodes: asymmetric clip.
    GermaniumDiode,
    /// No distortion curve: only the sag and the transformer colour.
    IronSag,
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
    /// 0..=1: power-supply sag. Loud passages lower the stage's headroom and
    /// gain a little, and it recovers over about a tenth of a second.
    pub sag: f32,
    /// 0..=1: output-transformer colour. The low bass saturates as the level
    /// rises, adding bass harmonics; mids and highs are untouched.
    pub transformer: f32,
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
            sag: 0.3,
            transformer: 0.3,
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
        self.sag = fin(self.sag, 0.3).clamp(0.0, 1.0);
        self.transformer = fin(self.transformer, 0.3).clamp(0.0, 1.0);
        self
    }
}

// ---------------------------------------------------------------------------
// The tube models (Koren)
// ---------------------------------------------------------------------------

/// Parameters of Koren's triode model (his SPICE library, fitted to
/// datasheets): mu, Ex, Kg1, Kp, Kvb, and the contact potential Vct.
#[derive(Clone, Copy)]
struct Koren {
    mu: f64,
    ex: f64,
    kg1: f64,
    kp: f64,
    kvb: f64,
    vct: f64,
}

/// Koren's pentode / beam-tetrode model ("PENTODE1" in his library): the
/// screen is held at a fixed voltage `vg2` and its current is ignored.
#[derive(Clone, Copy)]
struct KorenPentode {
    mu: f64,
    ex: f64,
    kg1: f64,
    kp: f64,
    kvb: f64,
    vg2: f64,
}

#[derive(Clone, Copy)]
enum Model {
    Triode(Koren),
    Pentode(KorenPentode),
}

impl Model {
    /// Plate current (amps).
    fn ip(&self, vg1: f64, vp: f64) -> f64 {
        match self {
            Model::Triode(k) => k.ip(vg1, vp),
            Model::Pentode(k) => {
                let z = (1.0 / k.mu + vg1 / k.vg2) * k.kp;
                let softplus = if z > 30.0 { z } else { (1.0 + z.exp()).ln() };
                let e1 = k.vg2 / k.kp * softplus;
                if e1 <= 0.0 || vp <= 0.0 { 0.0 } else { 2.0 * e1.powf(k.ex) / k.kg1 * (vp / k.kvb).atan() }
            }
        }
    }
}

impl Koren {
    /// Plate current (amps).
    fn ip(&self, vgk: f64, vpk: f64) -> f64 {
        let z = self.kp * (1.0 / self.mu + (vgk + self.vct) / (self.kvb + vpk * vpk).sqrt());
        let softplus = if z > 30.0 { z } else { (1.0 + z.exp()).ln() };
        let e1 = vpk / self.kp * softplus;
        if e1 <= 0.0 { 0.0 } else { 2.0 * e1.powf(self.ex) / self.kg1 }
    }
}

/// How the tube is operated.
#[derive(Clone, Copy)]
enum Operating {
    /// A resistor-loaded stage: supply `bplus`, load `r_load`. The grid bias
    /// is given, or (None) chosen to put the plate at half the supply.
    LoadLine { bplus: f64, r_load: f64, vgk: Option<f64> },
    /// A power stage at a stated operating point (plate volts and amps) into
    /// an AC load `r_ac` (an output transformer); the bias follows.
    Fixed { vp: f64, ip: f64, r_ac: f64 },
}

/// One tube driving the load, or a push-pull pair.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Topology {
    /// The output is the plate voltage along the load line.
    SingleEnded,
    /// Two tubes on opposite half-cycles; the output is the difference of their
    /// plate currents (biased for class AB, so the crossover region shows).
    PushPull,
}

/// A tube and the stage around it.
#[derive(Clone, Copy)]
struct TubeSpec {
    k: Model,
    topology: Topology,
    op: Operating,
    /// Grid-conduction resistance (ohms), against a 10 kΩ driving source.
    rgi: f64,
}

/// The resolved operating point: grid bias, plate volts and amps, AC load.
#[derive(Clone, Copy)]
struct Quiescent {
    vgk: f64,
    vp: f64,
    ip: f64,
    r_ac: f64,
}

/// Softness of the grid-conduction corner, in volts.
const GRID_KNEE: f64 = 0.05;
/// The source impedance that drives the grid.
const GRID_SOURCE_OHMS: f64 = 10_000.0;
/// Curve input beyond +-4 is held at the end value; table size.
const TABLE_RANGE: f64 = 4.0;
const TABLE_POINTS: usize = 4096;

const B_PLUS: f64 = 300.0; // the 12AX7 stage's supply (kept for the tests)
/// A push-pull pair is never perfectly matched: the second tube is this much
/// weaker, which leaves a little even harmonic.
const PAIR_MISMATCH: f64 = 0.94;
/// Class-AB pentode pairs are matched more closely.
const CLASS_AB_MISMATCH: f64 = 0.98;

/// Bisection on a monotonic function: `f(lo)` and `f(hi)` bracket zero.
fn bisect(mut lo: f64, mut hi: f64, f: impl Fn(f64) -> f64) -> f64 {
    let increasing = f(hi) > f(lo);
    for _ in 0..70 {
        let mid = 0.5 * (lo + hi);
        if (f(mid) > 0.0) == increasing {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    0.5 * (lo + hi)
}

impl TubeSpec {
    /// Plate voltage for a grid voltage on the load line through the
    /// operating point: `vp = vp_q - (Ip(vg, vp) - ip_q) * r_ac`.
    fn plate_voltage(&self, q: &Quiescent, vgk: f64) -> f64 {
        let vmax = q.vp + q.ip * q.r_ac;
        bisect(0.0, vmax, |vp| vp - (q.vp - (self.k.ip(vgk, vp) - q.ip) * q.r_ac))
    }

    fn quiescent(&self) -> Quiescent {
        match self.op {
            Operating::LoadLine { bplus, r_load, vgk } => {
                let plate_at = |vg: f64| {
                    // vp = bplus - Ip(vg, vp) * r_load
                    bisect(0.0, bplus, |vp| vp - (bplus - self.k.ip(vg, vp) * r_load))
                };
                let vgk = vgk.unwrap_or_else(|| bisect(-60.0, 0.0, |vg| bplus / 2.0 - plate_at(vg)));
                let vp = plate_at(vgk);
                Quiescent { vgk, vp, ip: (bplus - vp) / r_load, r_ac: r_load }
            }
            Operating::Fixed { vp, ip, r_ac } => {
                let vgk = bisect(-300.0, 5.0, |vg| self.k.ip(vg, vp) - ip);
                Quiescent { vgk, vp, ip, r_ac }
            }
        }
    }

    /// Grid voltage for a curve input `u`: the bias plus a swing equal to the
    /// bias (so `u = 1` reaches zero volts), with the positive half squashed
    /// by grid conduction.
    fn grid_voltage(&self, q: &Quiescent, u: f64) -> f64 {
        let k = self.rgi / (self.rgi + GRID_SOURCE_OHMS);
        let v = q.vgk + q.vgk.abs() * u;
        let sp = if v / GRID_KNEE > 40.0 { v } else { GRID_KNEE * (1.0 + (v / GRID_KNEE).exp()).ln() };
        v - (1.0 - k) * sp
    }
}

const fn koren(mu: f64, ex: f64, kg1: f64, kp: f64, kvb: f64, vct: f64) -> Model {
    Model::Triode(Koren { mu, ex, kg1, kp, kvb, vct })
}

const fn pentode(mu: f64, ex: f64, kg1: f64, kp: f64, kvb: f64, vg2: f64) -> Model {
    Model::Pentode(KorenPentode { mu, ex, kg1, kp, kvb, vg2 })
}

/// The tubes on offer. Parameters are Koren's datasheet fits from his tube
/// library; the stages around them are typical, not any one amplifier's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tube {
    Ax7,
    At7,
    Au7,
    Sn7,
    Dj8,
    T300b,
    A2a3,
    Sl7,
    Ay7,
    Ax7Syl,
    El84,
    El34Pp,
    L6Pp,
    Kt88Pp,
}

impl Tube {
    fn spec(self) -> TubeSpec {
        match self {
            // Koren's original 12AX7 fit; 300 V, 100 kΩ, -1.5 V bias (Phase 2).
            Tube::Ax7 => TubeSpec {
                k: koren(100.0, 1.4, 1060.0, 600.0, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: B_PLUS, r_load: 100_000.0, vgk: Some(-1.5) },
                rgi: 2_000.0,
            },
            // 12AT7 / ECC81 (Tom Mitchell fit).
            Tube::At7 => TubeSpec {
                k: koren(67.49, 1.234, 419.1, 213.96, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: 250.0, r_load: 47_000.0, vgk: None },
                rgi: 2_000.0,
            },
            // 12AU7 / ECC82 (Sylvania technical manual).
            Tube::Au7 => TubeSpec {
                k: koren(20.21, 1.230, 1108.7, 84.96, 551.3, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: 300.0, r_load: 47_000.0, vgk: None },
                rgi: 2_000.0,
            },
            // 6SN7 (Sylvania technical manual).
            Tube::Sn7 => TubeSpec {
                k: koren(21.07, 1.341, 1446.2, 157.81, 179.4, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: 300.0, r_load: 47_000.0, vgk: None },
                rgi: 2_000.0,
            },
            // 6DJ8 / ECC88 / 6922 (Tom Mitchell fit, with contact potential).
            Tube::Dj8 => TubeSpec {
                k: koren(30.51, 1.532, 453.9, 233.17, 190.9, 0.5),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: 200.0, r_load: 22_000.0, vgk: None },
                rgi: 2_000.0,
            },
            // 300B (Western Electric, 1950): about 300 V, 65 mA, 3.5 kΩ load.
            Tube::T300b => TubeSpec {
                k: koren(3.92, 1.504, 2140.3, 64.28, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::Fixed { vp: 300.0, ip: 0.065, r_ac: 3_500.0 },
                rgi: 1_000.0,
            },
            // 2A3 (Tung-Sol datasheet): about 250 V, 60 mA, 2.5 kΩ load.
            Tube::A2a3 => TubeSpec {
                k: koren(4.05, 1.634, 3652.2, 58.47, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::Fixed { vp: 250.0, ip: 0.060, r_ac: 2_500.0 },
                rgi: 1_000.0,
            },
            // 6SL7GT (GE): high-mu octal triode.
            Tube::Sl7 => TubeSpec {
                k: koren(75.89, 1.233, 1735.2, 1725.27, 7.0, 0.5),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: 300.0, r_load: 100_000.0, vgk: None },
                rgi: 2_000.0,
            },
            // 12AY7 (GE databook, 1955): low-noise, medium-mu triode.
            Tube::Ay7 => TubeSpec {
                k: koren(44.16, 1.113, 1192.4, 409.96, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: 300.0, r_load: 100_000.0, vgk: None },
                rgi: 2_000.0,
            },
            // 12AX7A (Sylvania technical manual, 1955).
            Tube::Ax7Syl => TubeSpec {
                k: koren(105.78, 1.474, 1618.2, 432.76, 35.6, 0.5),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine { bplus: 300.0, r_load: 100_000.0, vgk: None },
                rgi: 2_000.0,
            },
            // EL84 (Mullard), single-ended class A: 250 V, screen 250 V, 48 mA, 5.2 kΩ.
            Tube::El84 => TubeSpec {
                k: pentode(21.29, 1.240, 401.7, 111.04, 17.9, 250.0),
                topology: Topology::SingleEnded,
                op: Operating::Fixed { vp: 250.0, ip: 0.048, r_ac: 5_200.0 },
                rgi: 1_000.0,
            },
            // EL34 (Mullard, 1962) push-pull class AB: 400 V, screen 400 V,
            // 35 mA idle each, about 1.7 kΩ per tube.
            Tube::El34Pp => TubeSpec {
                k: pentode(12.02, 1.169, 353.9, 61.11, 29.9, 400.0),
                topology: Topology::PushPull,
                op: Operating::Fixed { vp: 400.0, ip: 0.035, r_ac: 1_650.0 },
                rgi: 1_000.0,
            },
            // 6L6GC (GE) push-pull class AB: 400 V, screen 400 V, 35 mA idle, 1 kΩ per tube.
            Tube::L6Pp => TubeSpec {
                k: pentode(9.88, 1.442, 1686.6, 30.98, 19.4, 400.0),
                topology: Topology::PushPull,
                op: Operating::Fixed { vp: 400.0, ip: 0.035, r_ac: 1_000.0 },
                rgi: 1_000.0,
            },
            // KT88 (M-O Valve) push-pull class AB: 450 V, screen 400 V, 50 mA idle, 1 kΩ per tube.
            Tube::Kt88Pp => TubeSpec {
                k: pentode(12.38, 1.246, 340.4, 26.48, 36.5, 400.0),
                topology: Topology::PushPull,
                op: Operating::Fixed { vp: 450.0, ip: 0.050, r_ac: 1_000.0 },
                rgi: 1_000.0,
            },
        }
    }

    /// The grid bias the stage settles at (tests).
    #[cfg(test)]
    fn bias(self) -> f64 {
        self.spec().quiescent().vgk
    }
}

/// Plate current (amps) of the 12AX7 model.
pub fn koren_plate_current(vgk: f64, vpk: f64) -> f64 {
    Tube::Ax7.spec().k.ip(vgk, vpk)
}

/// Plate voltage of the 12AX7 stage for a grid voltage.
pub fn triode_plate_voltage(vgk: f64) -> f64 {
    let spec = Tube::Ax7.spec();
    spec.plate_voltage(&spec.quiescent(), vgk)
}

/// Grid voltage of the 12AX7 stage for a curve input.
#[cfg(test)]
fn grid_voltage(u: f64) -> f64 {
    let spec = Tube::Ax7.spec();
    spec.grid_voltage(&spec.quiescent(), u)
}

/// A curve `f(u)`: zero at zero, unit slope at zero, tabulated once with its
/// antiderivative, so the audio path is a lookup and first-order antiderivative
/// antialiasing is exact for the interpolated curve.
pub struct TubeTable {
    lo: f64,
    step: f64,
    f: Vec<f64>,
    /// `anti[i]` = integral of the interpolated curve from `lo` to node i.
    anti: Vec<f64>,
}

impl TubeTable {
    /// Tabulate `raw` over the table range, then shift it to zero at zero
    /// and scale it to unit slope there.
    fn from_curve(raw: impl Fn(f64) -> f64) -> Self {
        let step = 2.0 * TABLE_RANGE / TABLE_POINTS as f64;
        let lo = -TABLE_RANGE;
        let raw: Vec<f64> = (0..=TABLE_POINTS).map(|i| raw(lo + i as f64 * step)).collect();
        let mid = TABLE_POINTS / 2;
        let slope = (raw[mid + 1] - raw[mid - 1]) / (2.0 * step);
        let f: Vec<f64> = raw.iter().map(|v| (v - raw[mid]) / slope).collect();
        let mut anti = vec![0.0; f.len()];
        for i in 1..f.len() {
            anti[i] = anti[i - 1] + 0.5 * step * (f[i - 1] + f[i]);
        }
        Self { lo, step, f, anti }
    }

    /// A tube stage as a curve (inverted, so polarity is kept).
    fn from_tube(tube: Tube) -> Self {
        let spec = tube.spec();
        let q = spec.quiescent();
        let current = |u: f64| {
            let vg = spec.grid_voltage(&q, u);
            spec.k.ip(vg, spec.plate_voltage(&q, vg))
        };
        match spec.topology {
            Topology::SingleEnded => Self::from_curve(|u| -spec.plate_voltage(&q, spec.grid_voltage(&q, u))),
            Topology::PushPull => Self::from_curve(|u| current(u) - CLASS_AB_MISMATCH * current(-u)),
        }
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
        let t = u - (self.lo + i as f64 * self.step);
        let f0 = self.f[i];
        let slope = (self.f[i + 1] - f0) / self.step;
        self.anti[i] + f0 * t + 0.5 * slope * t * t
    }
}

/// The shared 12AX7 table (built on first use).
pub fn triode_table() -> &'static TubeTable {
    tube_table(Tube::Ax7)
}

fn tube_table(tube: Tube) -> &'static TubeTable {
    static TABLES: [OnceLock<TubeTable>; 14] = [const { OnceLock::new() }; 14];
    let i = match tube {
        Tube::Ax7 => 0,
        Tube::At7 => 1,
        Tube::Au7 => 2,
        Tube::Sn7 => 3,
        Tube::Dj8 => 4,
        Tube::T300b => 5,
        Tube::A2a3 => 6,
        Tube::Sl7 => 7,
        Tube::Ay7 => 8,
        Tube::Ax7Syl => 9,
        Tube::El84 => 10,
        Tube::El34Pp => 11,
        Tube::L6Pp => 12,
        Tube::Kt88Pp => 13,
    };
    TABLES[i].get_or_init(|| TubeTable::from_tube(tube))
}

/// Class-A push-pull pair of 2A3s: the second tube sees the inverted signal
/// and the outputs subtract, which cancels even harmonics. A slight mismatch
/// (the second tube 8% weaker) leaves a little even content, as real pairs do.
fn push_pull_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| {
        let single = tube_table(Tube::A2a3);
        TubeTable::from_curve(|u| (single.eval(u) - PAIR_MISMATCH * single.eval(-u)) / (1.0 + PAIR_MISMATCH))
    })
}

/// A JFET stage: the square-law transfer `Id = Idss (1 - Vgs/Vp)^2`, biased at
/// half the pinch-off voltage, cut off on one side and clipped by gate
/// conduction on the other. Mostly 2nd harmonic, almost no 3rd.
fn jfet_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| {
        const VP: f64 = -2.0; // pinch-off
        const VQ: f64 = -1.0; // bias
        TubeTable::from_curve(|u| {
            let vgs = VQ + VQ.abs() * u;
            // Gate conduction above 0 V: the drive is squashed (soft corner).
            let knee = 0.05;
            let sp = if vgs / knee > 40.0 { vgs } else { knee * (1.0 + (vgs / knee).exp()).ln() };
            let vgs = vgs - 0.85 * sp;
            let x = (1.0 - vgs / VP).max(0.0);
            x * x
        })
    })
}

/// A silicon diode-pair clipper (antiparallel): a symmetric logarithmic soft
/// clip, `asinh(a u) / a`.
fn silicon_diode_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| TubeTable::from_curve(|u| (3.0 * u).asinh() / 3.0))
}

/// A germanium diode against a silicon one: the germanium side conducts at
/// about half the voltage, so that half clips earlier: asymmetric.
fn germanium_diode_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| TubeTable::from_curve(|u| if u >= 0.0 { (6.0 * u).asinh() / 6.0 } else { (3.0 * u).asinh() / 3.0 }))
}

/// A near-hard clip: `u / (1 + |u|^8)^(1/8)`, linear below about 0.8 and
/// flat above about 1.2, with a small rounded corner.
fn hard_transistor_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| TubeTable::from_curve(|u| u / (1.0 + u.abs().powi(8)).powf(1.0 / 8.0)))
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
    kind: ShaperKind,
}

#[derive(Clone, Copy)]
enum ShaperKind {
    Table(&'static TubeTable),
    Tanh,
    Linear,
}

impl Shaper {
    fn new(flavour: AnalogFlavour) -> Self {
        let table = |t| ShaperKind::Table(tube_table(t));
        let kind = match flavour {
            AnalogFlavour::WarmTriode => table(Tube::Ax7),
            AnalogFlavour::Tube12at7 => table(Tube::At7),
            AnalogFlavour::Tube12au7 => table(Tube::Au7),
            AnalogFlavour::Tube6sn7 => table(Tube::Sn7),
            AnalogFlavour::Tube6dj8 => table(Tube::Dj8),
            AnalogFlavour::Tube300b => table(Tube::T300b),
            AnalogFlavour::Tube2a3 => table(Tube::A2a3),
            AnalogFlavour::Tube6sl7 => table(Tube::Sl7),
            AnalogFlavour::Tube12ay7 => table(Tube::Ay7),
            AnalogFlavour::Tube12ax7a => table(Tube::Ax7Syl),
            AnalogFlavour::TubeEl84 => table(Tube::El84),
            AnalogFlavour::PushPullEl34 => table(Tube::El34Pp),
            AnalogFlavour::PushPull6l6gc => table(Tube::L6Pp),
            AnalogFlavour::PushPullKt88 => table(Tube::Kt88Pp),
            AnalogFlavour::PushPull => ShaperKind::Table(push_pull_table()),
            AnalogFlavour::HardTransistor => ShaperKind::Table(hard_transistor_table()),
            AnalogFlavour::Jfet => ShaperKind::Table(jfet_table()),
            AnalogFlavour::SiliconDiode => ShaperKind::Table(silicon_diode_table()),
            AnalogFlavour::GermaniumDiode => ShaperKind::Table(germanium_diode_table()),
            AnalogFlavour::SolidState => ShaperKind::Tanh,
            AnalogFlavour::IronSag => ShaperKind::Linear,
        };
        Self { kind }
    }

    #[inline]
    fn f(&self, u: f64) -> f64 {
        match self.kind {
            ShaperKind::Table(t) => t.eval(u),
            ShaperKind::Tanh => u.tanh(),
            ShaperKind::Linear => u,
        }
    }

    #[inline]
    fn anti(&self, u: f64) -> f64 {
        match self.kind {
            ShaperKind::Table(t) => t.antiderivative(u),
            ShaperKind::Tanh => ln_cosh(u),
            ShaperKind::Linear => 0.5 * u * u,
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

// Sag: an envelope of the driven level (fast attack, slower recovery) lowers
// the headroom (the stage is driven a little harder) and the output level.
const SAG_ATTACK_S: f32 = 0.005;
const SAG_RELEASE_S: f32 = 0.12;
/// At full sag and a saturated envelope: how much harder the curve is driven,
/// and how much quieter the stage gets.
const SAG_DRIVE: f32 = 0.5;
const SAG_LEVEL: f32 = 0.2;

/// What the sag envelope does at a level: `(extra drive, output factor)`.
#[inline]
fn sag_effect(sag: f32, env: f32) -> (f32, f32) {
    let sagf = sag * env / (1.0 + env);
    (1.0 + SAG_DRIVE * sagf, 1.0 - SAG_LEVEL * sagf)
}

// Transformer colour: the bass (below about 90 Hz) is soft-clipped, blended
// in by the amount; everything above passes through unchanged.
const XF_CORNER_HZ: f32 = 90.0;
const XF_HARDNESS: f32 = 6.0;

/// Gain that makes the processed level match the dry level for a -12 dBFS
/// RMS sine: what "auto gain match" applies.
fn gain_match(shaper: Shaper, g: f32, sag: f32) -> f32 {
    const N: usize = 2048;
    let amp = 0.354_f64; // -12 dBFS RMS
    // Steady-state sag envelope for this sine: mean |x| times the drive.
    let env = (amp * 2.0 / std::f64::consts::PI) as f32 * g;
    let (extra, level) = sag_effect(sag, env);
    let mut sum_in = 0.0f64;
    let mut ys = [0.0f64; N];
    for (i, y) in ys.iter_mut().enumerate() {
        let x = amp * (2.0 * std::f64::consts::PI * (i as f64) / 64.0).sin();
        sum_in += x * x;
        *y = shaper.f((g * extra) as f64 * x) / (g * extra) as f64 * level as f64;
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
    /// Sag envelope and the transformer's low-pass state.
    env: f32,
    lf: f32,
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
            env: 0.0,
            lf: 0.0,
        }
    }
    fn clear(&mut self, shaper: &Shaper) {
        self.xin.clear();
        self.yhr.clear();
        self.dry.clear();
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
        self.adaa = shaper.rest();
        self.env = 0.0;
        self.lf = 0.0;
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
    sag: f32,
    xf: f32,
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
            sag: 0.0,
            xf: 0.0,
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
        let comp = if s.auto_gain { gain_match(self.shaper, g, s.sag) } else { 1.0 };
        let out = 10f32.powf(s.output_db / 20.0);
        (g, comp, out, s.mix)
    }

    fn snap_params(&mut self) {
        let (g, comp, out, mix) = self.targets();
        self.g = g;
        self.comp = comp;
        self.out = out;
        self.mix = mix;
        self.sag = self.settings.sag;
        self.xf = self.settings.transformer;
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

    /// True once the stage is fully on: not fading in or out, no swap waiting.
    /// (While it is not, the wet and dry signals are still being mixed
    /// through the fade, so a level reading would mislead.)
    pub fn is_steady(&self) -> bool {
        self.settings.enabled && self.fade >= 1.0 && self.pending.is_none() && self.pending_plan.is_none()
    }

    /// True while the stage is processing (on, or fading out).
    pub fn is_active(&self) -> bool {
        self.settings.enabled || self.fade > 0.0
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
        let (tsag, txf) = (self.settings.sag, self.settings.transformer);
        let (mut sg, mut sc, mut so, mut sm, mut sf, mut ssag, mut sxf) = (
            (tg - self.g) / ramp,
            (tcomp - self.comp) / ramp,
            (tout - self.out) / ramp,
            (tmix - self.mix) / ramp,
            (target_fade - self.fade) / ramp,
            (tsag - self.sag) / ramp,
            (txf - self.xf) / ramp,
        );
        let sr = self.sample_rate.max(1) as f32;
        let (att, rel) = (1.0 - (-1.0 / (SAG_ATTACK_S * sr)).exp(), 1.0 - (-1.0 / (SAG_RELEASE_S * sr)).exp());
        let xf_a = 1.0 - (-2.0 * std::f32::consts::PI * XF_CORNER_HZ / sr).exp();
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
            step(&mut self.sag, tsag, &mut ssag);
            step(&mut self.xf, txf, &mut sxf);
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
                self.comp = if self.settings.auto_gain { gain_match(shaper, tg, tsag) } else { 1.0 };
                tcomp = self.comp;
                sc = 0.0;
                target_fade = if self.settings.enabled { 1.0 } else { 0.0 };
                sf = (target_fade - self.fade) / ramp;
            }
            let (comp, out, mix, fade) = (self.comp, self.out, self.mix, self.fade);
            let shaper = self.shaper;
            let g_now = self.g;
            let (sag, xf) = (self.sag, self.xf);

            for (ch, smp) in frame.iter_mut().enumerate() {
                let x = *smp;
                let st = &mut self.chans[ch];
                // Sag: follow the driven level, then drive harder / play quieter.
                let level = (x * g_now).abs();
                st.env += (level - st.env) * if level > st.env { att } else { rel };
                let (extra, sag_out) = sag_effect(sag, st.env);
                let gd = g_now * extra;
                let inv_g = 1.0 / gd; // unit small-signal gain at the driven level
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
                let wet = wet * sag_out;
                // Transformer colour: saturate the bass, blended in by the amount.
                st.lf += xf_a * (wet - st.lf);
                let lf_sat = (XF_HARDNESS * st.lf).tanh() / XF_HARDNESS;
                let wet = wet + xf * (lf_sat - st.lf);
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
        AnalogSettings { enabled: true, flavour, drive, mix, output_db: 0.0, auto_gain: false, antialias: AntiAliasChoice::Auto, sag: 0.0, transformer: 0.0 }
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

    fn colour(flavour: AnalogFlavour, drive: f32, sag: f32, transformer: f32) -> AnalogSettings {
        AnalogSettings { sag, transformer, ..on(flavour, drive, 1.0) }
    }

    /// RMS of the processed signal over `range` (samples of one channel).
    fn rms(x: &[f32], range: std::ops::Range<usize>) -> f64 {
        let w = &x[range];
        (w.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / w.len() as f64).sqrt()
    }

    #[test]
    fn sag_compresses_loud_passages_more_than_quiet_ones() {
        let (n, bin) = (1 << 14, 371); // ~1 kHz
        let gain_db = |sag: f32, amp: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(AnalogFlavour::WarmTriode, 0.3, sag, 0.0));
            let out = run(&mut st, bin, n, amp);
            let o = (out.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
            db(o / (amp as f64 / 2f64.sqrt()))
        };
        let squash = |sag| gain_db(sag, 0.1) - gain_db(sag, 0.8);
        assert!(squash(1.0) > squash(0.0) + 0.5, "sag adds compression: {:.2} dB vs {:.2} dB", squash(1.0), squash(0.0));
    }

    #[test]
    fn sag_recovers_over_about_a_tenth_of_a_second() {
        let fs = 44_100usize;
        let quiet_level_after = |sag: f32, start_ms: usize| {
            let mut st = AnalogStage::new(fs as u32);
            st.set_settings(colour(AnalogFlavour::SolidState, 0.5, sag, 0.0));
            // 300 ms loud burst, then a quiet tone.
            let sr = fs as f64;
            let mut x: Vec<f32> = (0..fs / 3 * 2).map(|i| if i < fs * 3 / 10 { 0.9 } else { 0.1 } * (2.0 * PI * 1000.0 * i as f64 / sr).sin() as f32).collect();
            st.process(&mut x, 1);
            let after = fs * 3 / 10 + fs / 100; // skip the wet-path latency and the fade
            rms(&x, after + start_ms * fs / 1000..after + start_ms * fs / 1000 + fs / 50)
        };
        let drop_db = |sag: f32| db(quiet_level_after(sag, 40) / quiet_level_after(sag, 180));
        assert!(drop_db(1.0) < -0.3, "just after the burst the stage is quieter: {:.2} dB", drop_db(1.0));
        assert!(drop_db(0.0).abs() < 0.1, "no sag, no recovery curve: {:.2} dB", drop_db(0.0));
    }

    #[test]
    fn transformer_saturates_the_bass_with_level_and_leaves_the_mids_alone() {
        let n = 1 << 14;
        let h3 = |bin: usize, amp: f32, xf: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(AnalogFlavour::SolidState, 0.0, 0.0, xf));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            db(sp[bin * 3] / sp[bin])
        };
        let (bass, mid) = (22, 371); // ~59 Hz and ~1 kHz
        assert!(h3(bass, 0.3, 1.0) > h3(bass, 0.3, 0.0) + 8.0, "bass harmonics appear with the transformer on");
        assert!(h3(bass, 0.6, 1.0) > h3(bass, 0.1, 1.0) + 12.0, "and grow with level");
        assert!(h3(bass, 0.3, 1.0) > h3(bass, 0.3, 0.3), "and with the amount");
        assert!((h3(mid, 0.3, 1.0) - h3(mid, 0.3, 0.0)).abs() < 3.0, "1 kHz is unaffected");
    }

    #[test]
    fn linear_response_stays_flat_at_low_level_with_sag_and_transformer_on() {
        // Small signals see no colour: within +-1 dB from 30 Hz to 16 kHz, both flavours.
        let n = 1 << 14;
        for flavour in [AnalogFlavour::WarmTriode, AnalogFlavour::SolidState] {
            for hz in [30.0, 60.0, 120.0, 500.0, 2_000.0, 8_000.0, 16_000.0] {
                let bin = (hz / 44_100.0 * n as f64).round() as usize;
                let mut st = AnalogStage::new(44_100);
                st.set_settings(colour(flavour, 0.0, 0.3, 0.3));
                let sp = spectrum(&run(&mut st, bin, n, 0.01));
                let gain = db(sp[bin] / 0.01);
                assert!(gain.abs() < 1.0, "{flavour:?} at {hz} Hz: {gain:.2} dB");
            }
        }
    }

    #[test]
    fn sag_and_transformer_changes_do_not_click() {
        let (n, bin) = (1 << 13, 12); // ~65 Hz bass tone
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::WarmTriode, 0.5, 0.0, 0.0));
        let mut a = tone(bin, n, 2, 0.4, 1);
        st.process(&mut a, 1);
        st.set_settings(colour(AnalogFlavour::WarmTriode, 0.5, 1.0, 1.0));
        let mut b = tone(bin, n, 2, 0.4, 1);
        st.process(&mut b, 1);
        let seam = [a, b].concat();
        assert!(max_step(&seam[n..], 1) < 0.05, "no click when sag and transformer change live");
    }

    #[test]
    fn colour_settings_are_clamped_and_default_in() {
        let s = AnalogSettings { sag: 3.0, transformer: -1.0, ..Default::default() }.clamped();
        assert_eq!((s.sag, s.transformer), (1.0, 0.0));
        let parsed: AnalogSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!((parsed.sag, parsed.transformer), (0.3, 0.3));
    }

    const ALL_FLAVOURS: [AnalogFlavour; 21] = [
        AnalogFlavour::WarmTriode,
        AnalogFlavour::PushPull,
        AnalogFlavour::SolidState,
        AnalogFlavour::HardTransistor,
        AnalogFlavour::Tube12at7,
        AnalogFlavour::Tube12au7,
        AnalogFlavour::Tube6sn7,
        AnalogFlavour::Tube6dj8,
        AnalogFlavour::Tube300b,
        AnalogFlavour::Tube2a3,
        AnalogFlavour::Tube6sl7,
        AnalogFlavour::Tube12ay7,
        AnalogFlavour::Tube12ax7a,
        AnalogFlavour::TubeEl84,
        AnalogFlavour::PushPullEl34,
        AnalogFlavour::PushPull6l6gc,
        AnalogFlavour::PushPullKt88,
        AnalogFlavour::Jfet,
        AnalogFlavour::SiliconDiode,
        AnalogFlavour::GermaniumDiode,
        AnalogFlavour::IronSag,
    ];

    const ALL_TUBES: [Tube; 14] = [
        Tube::Ax7, Tube::At7, Tube::Au7, Tube::Sn7, Tube::Dj8, Tube::T300b, Tube::A2a3,
        Tube::Sl7, Tube::Ay7, Tube::Ax7Syl, Tube::El84, Tube::El34Pp, Tube::L6Pp, Tube::Kt88Pp,
    ];

    #[test]
    fn every_tube_has_a_sane_operating_point() {
        for t in ALL_TUBES {
            let sp = t.spec();
            let q = sp.quiescent();
            assert!(q.vgk < 0.0 && q.ip > 0.0 && q.vp > 0.0, "{t:?}: bias {:.1} V, {:.2} mA at {:.0} V", q.vgk, q.ip * 1000.0, q.vp);
            let check = sp.k.ip(q.vgk, q.vp);
            assert!((check - q.ip).abs() / q.ip < 0.01, "{t:?}: the bias reproduces the stated current ({:.2} vs {:.2} mA)", check * 1000.0, q.ip * 1000.0);
            // Plate voltage swings the right way and stays on the supply side of the load line.
            let (hi, lo) = (sp.plate_voltage(&q, q.vgk - 3.0 * q.vgk.abs()), sp.plate_voltage(&q, 0.0));
            assert!(hi > q.vp && q.vp > lo && lo >= 0.0, "{t:?}: plate {lo:.0} < {:.0} < {hi:.0} V", q.vp);
        }
        // Published power-triode biases: 300B about -62 V at 300 V / 65 mA; 2A3 about -45 V at 250 V / 60 mA.
        assert!((-66.0..-56.0).contains(&Tube::T300b.bias()), "300B bias {:.1} V", Tube::T300b.bias());
        assert!((-50.0..-40.0).contains(&Tube::A2a3.bias()), "2A3 bias {:.1} V", Tube::A2a3.bias());
        // EL84 single-ended class A (Mullard): 250 V, screen 250 V, 48 mA gives about -7.3 V.
        assert!((Tube::El84.bias() + 7.3).abs() < 1.0, "EL84 bias {:.1} V", Tube::El84.bias());
    }

    #[test]
    fn every_curve_is_a_unit_slope_monotonic_table() {
        for f in ALL_FLAVOURS {
            let sh = Shaper::new(f);
            let slope = (sh.f(0.001) - sh.f(-0.001)) / 0.002;
            assert!((slope - 1.0).abs() < 0.01, "{f:?}: unit slope, got {slope}");
            assert!(sh.f(0.0).abs() < 1e-9, "{f:?}: zero at zero");
            let mut prev = f64::MIN;
            for i in -400..=400 {
                let v = sh.f(i as f64 * 0.01);
                assert!(v >= prev - 1e-9, "{f:?}: non-decreasing at {}", i as f64 * 0.01);
                prev = v;
            }
        }
    }

    #[test]
    fn single_ended_tubes_are_even_dominant_and_push_pull_is_odd_dominant() {
        let (n, bin) = (1 << 14, 200);
        let h = |f: AnalogFlavour, amp: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(f, 0.4, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
        };
        for f in [AnalogFlavour::WarmTriode, AnalogFlavour::Tube12at7, AnalogFlavour::Tube12au7, AnalogFlavour::Tube6sn7, AnalogFlavour::Tube6dj8, AnalogFlavour::Tube300b, AnalogFlavour::Tube2a3, AnalogFlavour::Tube6sl7, AnalogFlavour::Tube12ay7, AnalogFlavour::Tube12ax7a] {
            let (h2, h3) = h(f, 0.2);
            assert!(h2 > h3 + 15.0, "{f:?}: 2nd {h2:.1} dB well above 3rd {h3:.1} dB");
            assert!(h2 < -20.0 && h2 > -60.0, "{f:?}: audible but not extreme 2nd: {h2:.1} dB");
        }
        let (h2, h3) = h(AnalogFlavour::PushPull, 0.3);
        assert!(h3 > h2 + 8.0, "push-pull: 3rd {h3:.1} dB above 2nd {h2:.1} dB");
        assert!(h2 > -100.0, "...with a little even left from the imperfect match ({h2:.1} dB)");
        // ...and it is much cleaner in the even harmonics than a single-ended 2A3.
        assert!(h2 < h(AnalogFlavour::Tube2a3, 0.3).0 - 15.0, "push-pull cancels the 2nd of its own tube");
    }

    #[test]
    fn class_ab_pentode_pairs_are_odd_dominant_with_crossover_grit_at_low_level() {
        let (n, bin) = (1 << 14, 200);
        let h = |f: AnalogFlavour, amp: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(f, 0.4, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
        };
        let class_a = h(AnalogFlavour::PushPull, 0.1).1;
        for f in [AnalogFlavour::PushPullEl34, AnalogFlavour::PushPull6l6gc, AnalogFlavour::PushPullKt88] {
            let (h2, h3) = h(f, 0.3);
            assert!(h3 > h2 + 8.0, "{f:?}: odd-dominant, 3rd {h3:.1} dB vs 2nd {h2:.1} dB");
            // Class AB leaves a crossover region: the 3rd is already there at low level,
            // much more than in the class-A pair of 2A3s.
            let low = h(f, 0.1).1;
            assert!(low > class_a + 15.0, "{f:?}: crossover distortion at low level ({low:.1} dB vs {class_a:.1} dB)");
        }
    }

    #[test]
    fn single_ended_pentode_has_both_kinds_of_harmonic() {
        let (n, bin) = (1 << 14, 200);
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::TubeEl84, 0.4, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, 0.1));
        let (h2, h3) = (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]));
        assert!(h2 > -45.0 && h3 > -60.0, "a class-A pentode is not clean: 2nd {h2:.1}, 3rd {h3:.1} dB");
    }

    #[test]
    fn jfet_is_square_law_and_diodes_clip_symmetrically_or_not() {
        let (n, bin) = (1 << 14, 200);
        let h = |f: AnalogFlavour, amp: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(f, 0.4, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
        };
        let (h2, h3) = h(AnalogFlavour::Jfet, 0.2);
        assert!(h2 > -30.0 && h2 < -12.0, "JFET 2nd {h2:.1} dB");
        assert!(h2 > h3 + 40.0, "square law: almost no 3rd ({h3:.1} dB vs {h2:.1} dB)");
        let (s2, s3) = h(AnalogFlavour::SiliconDiode, 0.2);
        assert!(s2 < -80.0 && s3 > -45.0, "silicon pair is symmetric: 2nd {s2:.1}, 3rd {s3:.1} dB");
        let (g2, g3) = h(AnalogFlavour::GermaniumDiode, 0.1);
        assert!(g2 > -40.0 && g3 > -45.0, "germanium against silicon is lopsided: 2nd {g2:.1}, 3rd {g3:.1} dB");
    }

    #[test]
    fn iron_and_sag_only_has_no_distortion_curve_but_still_colours_the_bass_and_dynamics() {
        let (n, bin) = (1 << 14, 371);
        let mut st = AnalogStage::new(44_100);
        st.set_settings(colour(AnalogFlavour::IronSag, 0.7, 0.0, 0.0));
        let sp = spectrum(&run(&mut st, bin, n, 0.6));
        assert!(db(sp[bin * 3] / sp[bin]) < -100.0 && db(sp[bin * 2] / sp[bin]) < -100.0, "a linear curve adds no harmonics");
        // The transformer still saturates the bass.
        let h3 = |xf: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(AnalogFlavour::IronSag, 0.0, 0.0, xf));
            let sp = spectrum(&run(&mut st, 22, n, 0.5));
            db(sp[66] / sp[22])
        };
        assert!(h3(1.0) > h3(0.0) + 30.0, "bass harmonics from the transformer alone");
    }

    #[test]
    fn hard_transistor_is_clean_below_the_knee_and_harsh_above_it() {
        let (n, bin) = (1 << 14, 200);
        let h3 = |amp: f32, drive: f32| {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(AnalogFlavour::HardTransistor, drive, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, amp));
            (db(sp[bin * 2] / sp[bin]), db(sp[bin * 3] / sp[bin]))
        };
        assert!(h3(0.1, 0.4).1 < -90.0, "clean well below the knee");
        let (h2, h3_hard) = h3(0.6, 0.4);
        assert!(h3_hard > -30.0, "strong 3rd once it clips ({h3_hard:.1} dB)");
        assert!(h2 < -80.0, "symmetric: no even harmonics ({h2:.1} dB)");
        // Harsher than the soft symmetric curve at the same setting.
        let soft = {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(AnalogFlavour::SolidState, 0.4, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, 0.3));
            db(sp[bin * 5] / sp[bin])
        };
        let hard = {
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(AnalogFlavour::HardTransistor, 0.4, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, 0.75));
            db(sp[bin * 5] / sp[bin])
        };
        assert!(hard > soft, "the hard clip's 5th ({hard:.1} dB) exceeds the soft one's at a lower level ({soft:.1} dB)");
    }

    #[test]
    fn every_flavour_keeps_aliasing_low_at_every_rate_and_is_linear_when_quiet() {
        for f in ALL_FLAVOURS {
            // 96 kHz (2x oversampling) is measured for every flavour in the ignored
            // profile test and for the reference flavour above; 44.1 and 192 kHz here.
            for fs in [44_100u32, 192_000] {
                let a = alias_db_with(fs, 9_500.0, 0.53, 0.8, f, None);
                assert!(a < -60.0, "{f:?} at {fs} Hz aliases at {a:.1} dB");
            }
            let n = 1 << 14;
            let bin = 371;
            let mut st = AnalogStage::new(44_100);
            st.set_settings(colour(f, 0.0, 0.0, 0.0));
            let sp = spectrum(&run(&mut st, bin, n, 0.01));
            assert!(db(sp[bin] / 0.01).abs() < 1.0, "{f:?}: unity gain for a quiet signal");
        }
    }

    #[test]
    fn flavour_names_round_trip_and_every_flavour_can_be_selected_live() {
        for (f, name) in [
            (AnalogFlavour::WarmTriode, "warm_triode"),
            (AnalogFlavour::PushPull, "push_pull"),
            (AnalogFlavour::SolidState, "solid_state"),
            (AnalogFlavour::HardTransistor, "hard_transistor"),
            (AnalogFlavour::Tube12at7, "tube_12at7"),
            (AnalogFlavour::Tube12au7, "tube_12au7"),
            (AnalogFlavour::Tube6sn7, "tube_6sn7"),
            (AnalogFlavour::Tube6dj8, "tube_6dj8"),
            (AnalogFlavour::Tube300b, "tube_300b"),
            (AnalogFlavour::Tube2a3, "tube_2a3"),
            (AnalogFlavour::Tube6sl7, "tube_6sl7"),
            (AnalogFlavour::Tube12ay7, "tube_12ay7"),
            (AnalogFlavour::Tube12ax7a, "tube_12ax7a"),
            (AnalogFlavour::TubeEl84, "tube_el84"),
            (AnalogFlavour::PushPullEl34, "push_pull_el34"),
            (AnalogFlavour::PushPull6l6gc, "push_pull_6l6gc"),
            (AnalogFlavour::PushPullKt88, "push_pull_kt88"),
            (AnalogFlavour::Jfet, "jfet"),
            (AnalogFlavour::SiliconDiode, "silicon_diode"),
            (AnalogFlavour::GermaniumDiode, "germanium_diode"),
            (AnalogFlavour::IronSag, "iron_sag"),
        ] {
            assert_eq!(serde_json::to_string(&f).unwrap(), format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<AnalogFlavour>(&format!("\"{name}\"")).unwrap(), f);
        }
        // Step through every flavour while audio flows: each swap fades, nothing clicks or blows up.
        let (n, bin) = (1 << 12, 40);
        let mut st = AnalogStage::new(44_100);
        let mut all = Vec::new();
        for f in ALL_FLAVOURS {
            st.set_settings(colour(f, 0.5, 0.2, 0.2));
            let mut x = tone(bin, n, 1, 0.4, 1);
            st.process(&mut x, 1);
            assert!(x.iter().all(|v| v.is_finite() && v.abs() < 2.0), "{f:?}: finite and bounded");
            all.extend(x);
        }
        assert!(max_step(&all[n..], 1) < 0.1, "no click while stepping through the flavours");
    }

    /// Prints each flavour's operating point and harmonic profile (run with
    /// --ignored --nocapture); section 15 of Analog-Emulation.md comes from this.
    #[test]
    #[ignore]
    fn print_flavour_profiles() {
        for t in ALL_TUBES {
            let sp = t.spec();
            let q = sp.quiescent();
            println!("{:?}: bias {:.1} V, plate {:.0} V, {:.1} mA, load {:.0} ohm", t, q.vgk, q.vp, q.ip * 1000.0, q.r_ac);
        }
        let (n, bin) = (1 << 14, 200);
        for f in ALL_FLAVOURS {
            let mut line = format!("{f:?}:");
            for amp in [0.1f32, 0.3, 0.6] {
                let mut st = AnalogStage::new(44_100);
                st.set_settings(colour(f, 0.4, 0.0, 0.0));
                let sp = spectrum(&run(&mut st, bin, n, amp));
                let d = |k: usize| db(sp[bin * k] / sp[bin]);
                line += &format!("  in {amp}: 2nd {:6.1} 3rd {:6.1} 4th {:6.1} 5th {:6.1} |", d(2), d(3), d(4), d(5));
            }
            println!("{line}");
        }
        for f in ALL_FLAVOURS {
            let a = |fs: u32| alias_db_with(fs, 9_500.0, 0.53, 0.8, f, None);
            println!("ALIAS {f:?}: 44.1k {:.1}  96k {:.1}  192k {:.1}", a(44_100), a(96_000), a(192_000));
        }
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
