//! The device models: Koren's tube equations, each tube's operating point,
//! and the transfer tables built from them (tubes, JFET, diodes, transistor).

use std::sync::OnceLock;

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
                if e1 <= 0.0 || vp <= 0.0 {
                    0.0
                } else {
                    2.0 * e1.powf(k.ex) / k.kg1 * (vp / k.kvb).atan()
                }
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
        if e1 <= 0.0 {
            0.0
        } else {
            2.0 * e1.powf(self.ex) / self.kg1
        }
    }
}

/// How the tube is operated.
#[derive(Clone, Copy)]
enum Operating {
    /// A resistor-loaded stage: supply `bplus`, load `r_load`. The grid bias
    /// is given, or (None) chosen to put the plate at half the supply.
    LoadLine {
        bplus: f64,
        r_load: f64,
        vgk: Option<f64>,
    },
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
pub(super) struct TubeSpec {
    k: Model,
    topology: Topology,
    op: Operating,
    /// Grid-conduction resistance (ohms), against a 10 kΩ driving source.
    rgi: f64,
}

/// The resolved operating point: grid bias, plate volts and amps, AC load.
#[derive(Clone, Copy)]
pub(super) struct Quiescent {
    pub(super) vgk: f64,
    pub(super) vp: f64,
    pub(super) ip: f64,
    pub(super) r_ac: f64,
}

/// Softness of the grid-conduction corner, in volts.
const GRID_KNEE: f64 = 0.05;
/// The source impedance that drives the grid.
const GRID_SOURCE_OHMS: f64 = 10_000.0;
/// Curve input beyond +-4 is held at the end value; table size.
pub(super) const TABLE_RANGE: f64 = 4.0;
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
        bisect(0.0, vmax, |vp| {
            vp - (q.vp - (self.k.ip(vgk, vp) - q.ip) * q.r_ac)
        })
    }

    pub(super) fn quiescent(&self) -> Quiescent {
        match self.op {
            Operating::LoadLine { bplus, r_load, vgk } => {
                let plate_at = |vg: f64| {
                    // vp = bplus - Ip(vg, vp) * r_load
                    bisect(0.0, bplus, |vp| vp - (bplus - self.k.ip(vg, vp) * r_load))
                };
                let vgk =
                    vgk.unwrap_or_else(|| bisect(-60.0, 0.0, |vg| bplus / 2.0 - plate_at(vg)));
                let vp = plate_at(vgk);
                Quiescent {
                    vgk,
                    vp,
                    ip: (bplus - vp) / r_load,
                    r_ac: r_load,
                }
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
        let sp = if v / GRID_KNEE > 40.0 {
            v
        } else {
            GRID_KNEE * (1.0 + (v / GRID_KNEE).exp()).ln()
        };
        v - (1.0 - k) * sp
    }
}

const fn koren(mu: f64, ex: f64, kg1: f64, kp: f64, kvb: f64, vct: f64) -> Model {
    Model::Triode(Koren {
        mu,
        ex,
        kg1,
        kp,
        kvb,
        vct,
    })
}

const fn pentode(mu: f64, ex: f64, kg1: f64, kp: f64, kvb: f64, vg2: f64) -> Model {
    Model::Pentode(KorenPentode {
        mu,
        ex,
        kg1,
        kp,
        kvb,
        vg2,
    })
}

/// The tubes on offer. Parameters are Koren's datasheet fits from his tube
/// library; the stages around them are typical, not any one amplifier's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tube {
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
    pub(super) fn spec(self) -> TubeSpec {
        match self {
            // Koren's original 12AX7 fit; 300 V, 100 kΩ, -1.5 V bias (Phase 2).
            Tube::Ax7 => TubeSpec {
                k: koren(100.0, 1.4, 1060.0, 600.0, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: B_PLUS,
                    r_load: 100_000.0,
                    vgk: Some(-1.5),
                },
                rgi: 2_000.0,
            },
            // 12AT7 / ECC81 (Tom Mitchell fit).
            Tube::At7 => TubeSpec {
                k: koren(67.49, 1.234, 419.1, 213.96, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: 250.0,
                    r_load: 47_000.0,
                    vgk: None,
                },
                rgi: 2_000.0,
            },
            // 12AU7 / ECC82 (Sylvania technical manual).
            Tube::Au7 => TubeSpec {
                k: koren(20.21, 1.230, 1108.7, 84.96, 551.3, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: 300.0,
                    r_load: 47_000.0,
                    vgk: None,
                },
                rgi: 2_000.0,
            },
            // 6SN7 (Sylvania technical manual).
            Tube::Sn7 => TubeSpec {
                k: koren(21.07, 1.341, 1446.2, 157.81, 179.4, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: 300.0,
                    r_load: 47_000.0,
                    vgk: None,
                },
                rgi: 2_000.0,
            },
            // 6DJ8 / ECC88 / 6922 (Tom Mitchell fit, with contact potential).
            Tube::Dj8 => TubeSpec {
                k: koren(30.51, 1.532, 453.9, 233.17, 190.9, 0.5),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: 200.0,
                    r_load: 22_000.0,
                    vgk: None,
                },
                rgi: 2_000.0,
            },
            // 300B (Western Electric, 1950): about 300 V, 65 mA, 3.5 kΩ load.
            Tube::T300b => TubeSpec {
                k: koren(3.92, 1.504, 2140.3, 64.28, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::Fixed {
                    vp: 300.0,
                    ip: 0.065,
                    r_ac: 3_500.0,
                },
                rgi: 1_000.0,
            },
            // 2A3 (Tung-Sol datasheet): about 250 V, 60 mA, 2.5 kΩ load.
            Tube::A2a3 => TubeSpec {
                k: koren(4.05, 1.634, 3652.2, 58.47, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::Fixed {
                    vp: 250.0,
                    ip: 0.060,
                    r_ac: 2_500.0,
                },
                rgi: 1_000.0,
            },
            // 6SL7GT (GE): high-mu octal triode.
            Tube::Sl7 => TubeSpec {
                k: koren(75.89, 1.233, 1735.2, 1725.27, 7.0, 0.5),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: 300.0,
                    r_load: 100_000.0,
                    vgk: None,
                },
                rgi: 2_000.0,
            },
            // 12AY7 (GE databook, 1955): low-noise, medium-mu triode.
            Tube::Ay7 => TubeSpec {
                k: koren(44.16, 1.113, 1192.4, 409.96, 300.0, 0.0),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: 300.0,
                    r_load: 100_000.0,
                    vgk: None,
                },
                rgi: 2_000.0,
            },
            // 12AX7A (Sylvania technical manual, 1955).
            Tube::Ax7Syl => TubeSpec {
                k: koren(105.78, 1.474, 1618.2, 432.76, 35.6, 0.5),
                topology: Topology::SingleEnded,
                op: Operating::LoadLine {
                    bplus: 300.0,
                    r_load: 100_000.0,
                    vgk: None,
                },
                rgi: 2_000.0,
            },
            // EL84 (Mullard), single-ended class A: 250 V, screen 250 V, 48 mA, 5.2 kΩ.
            Tube::El84 => TubeSpec {
                k: pentode(21.29, 1.240, 401.7, 111.04, 17.9, 250.0),
                topology: Topology::SingleEnded,
                op: Operating::Fixed {
                    vp: 250.0,
                    ip: 0.048,
                    r_ac: 5_200.0,
                },
                rgi: 1_000.0,
            },
            // EL34 (Mullard, 1962) push-pull class AB: 400 V, screen 400 V,
            // 35 mA idle each, about 1.7 kΩ per tube.
            Tube::El34Pp => TubeSpec {
                k: pentode(12.02, 1.169, 353.9, 61.11, 29.9, 400.0),
                topology: Topology::PushPull,
                op: Operating::Fixed {
                    vp: 400.0,
                    ip: 0.035,
                    r_ac: 1_650.0,
                },
                rgi: 1_000.0,
            },
            // 6L6GC (GE) push-pull class AB: 400 V, screen 400 V, 35 mA idle, 1 kΩ per tube.
            Tube::L6Pp => TubeSpec {
                k: pentode(9.88, 1.442, 1686.6, 30.98, 19.4, 400.0),
                topology: Topology::PushPull,
                op: Operating::Fixed {
                    vp: 400.0,
                    ip: 0.035,
                    r_ac: 1_000.0,
                },
                rgi: 1_000.0,
            },
            // KT88 (M-O Valve) push-pull class AB: 450 V, screen 400 V, 50 mA idle, 1 kΩ per tube.
            Tube::Kt88Pp => TubeSpec {
                k: pentode(12.38, 1.246, 340.4, 26.48, 36.5, 400.0),
                topology: Topology::PushPull,
                op: Operating::Fixed {
                    vp: 450.0,
                    ip: 0.050,
                    r_ac: 1_000.0,
                },
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
        let raw: Vec<f64> = (0..=TABLE_POINTS)
            .map(|i| raw(lo + i as f64 * step))
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

    /// A tube stage as a curve (inverted, so polarity is kept).
    fn from_tube(tube: Tube) -> Self {
        let spec = tube.spec();
        let q = spec.quiescent();
        let current = |u: f64| {
            let vg = spec.grid_voltage(&q, u);
            spec.k.ip(vg, spec.plate_voltage(&q, vg))
        };
        match spec.topology {
            Topology::SingleEnded => {
                Self::from_curve(|u| -spec.plate_voltage(&q, spec.grid_voltage(&q, u)))
            }
            Topology::PushPull => {
                Self::from_curve(|u| current(u) - CLASS_AB_MISMATCH * current(-u))
            }
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
            return self.anti[self.f.len() - 1]
                + self.f[self.f.len() - 1] * (u - (self.lo + last * self.step));
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

pub(super) fn tube_table(tube: Tube) -> &'static TubeTable {
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
pub(super) fn push_pull_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| {
        let single = tube_table(Tube::A2a3);
        TubeTable::from_curve(|u| {
            (single.eval(u) - PAIR_MISMATCH * single.eval(-u)) / (1.0 + PAIR_MISMATCH)
        })
    })
}

/// A JFET stage: the square-law transfer `Id = Idss (1 - Vgs/Vp)^2`, biased at
/// half the pinch-off voltage, cut off on one side and clipped by gate
/// conduction on the other. Mostly 2nd harmonic, almost no 3rd.
pub(super) fn jfet_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| {
        const VP: f64 = -2.0; // pinch-off
        const VQ: f64 = -1.0; // bias
        TubeTable::from_curve(|u| {
            let vgs = VQ + VQ.abs() * u;
            // Gate conduction above 0 V: the drive is squashed (soft corner).
            let knee = 0.05;
            let sp = if vgs / knee > 40.0 {
                vgs
            } else {
                knee * (1.0 + (vgs / knee).exp()).ln()
            };
            let vgs = vgs - 0.85 * sp;
            let x = (1.0 - vgs / VP).max(0.0);
            x * x
        })
    })
}

/// A silicon diode-pair clipper (antiparallel): a symmetric logarithmic soft
/// clip, `asinh(a u) / a`.
pub(super) fn silicon_diode_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| TubeTable::from_curve(|u| (3.0 * u).asinh() / 3.0))
}

/// A germanium diode against a silicon one: the germanium side conducts at
/// about half the voltage, so that half clips earlier: asymmetric.
pub(super) fn germanium_diode_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| {
        TubeTable::from_curve(|u| {
            if u >= 0.0 {
                (6.0 * u).asinh() / 6.0
            } else {
                (3.0 * u).asinh() / 3.0
            }
        })
    })
}

/// A near-hard clip: `u / (1 + |u|^8)^(1/8)`, linear below about 0.8 and
/// flat above about 1.2, with a small rounded corner.
pub(super) fn hard_transistor_table() -> &'static TubeTable {
    static T: OnceLock<TubeTable> = OnceLock::new();
    T.get_or_init(|| TubeTable::from_curve(|u| u / (1.0 + u.abs().powi(8)).powf(1.0 / 8.0)))
}

#[cfg(test)]
pub(super) const ALL_TUBES: [Tube; 14] = [
    Tube::Ax7,
    Tube::At7,
    Tube::Au7,
    Tube::Sn7,
    Tube::Dj8,
    Tube::T300b,
    Tube::A2a3,
    Tube::Sl7,
    Tube::Ay7,
    Tube::Ax7Syl,
    Tube::El84,
    Tube::El34Pp,
    Tube::L6Pp,
    Tube::Kt88Pp,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn koren_model_gives_plausible_12ax7_currents() {
        // Datasheet-scale checks (RCA bogey: about 1.2 mA at 250 V, -2 V grid).
        let ip = koren_plate_current(-2.0, 250.0) * 1000.0;
        assert!((0.7..1.6).contains(&ip), "Ip(-2 V, 250 V) = {ip:.2} mA");
        assert!(
            koren_plate_current(-6.0, 250.0) < 1e-5,
            "cut off well below the bias"
        );
        let (a, b, c) = (
            koren_plate_current(-3.0, 250.0),
            koren_plate_current(-2.0, 250.0),
            koren_plate_current(-1.0, 250.0),
        );
        assert!(a < b && b < c, "more current as the grid rises");
        let (p1, p2) = (triode_plate_voltage(-3.0), triode_plate_voltage(-0.5));
        assert!(
            p1 > p2 && p1 < B_PLUS && p2 > 0.0,
            "plate voltage falls as the grid rises: {p1:.0} V -> {p2:.0} V"
        );
    }

    #[test]
    fn tube_table_is_a_unit_slope_asymmetric_curve() {
        let t = triode_table();
        assert!(t.eval(0.0).abs() < 1e-9, "zero at zero");
        let slope = (t.eval(0.001) - t.eval(-0.001)) / 0.002;
        assert!(
            (slope - 1.0).abs() < 0.01,
            "unit small-signal slope, got {slope}"
        );
        let mut prev = f64::MIN;
        for i in -400..=400 {
            let v = t.eval(i as f64 * 0.01);
            assert!(v >= prev - 1e-12, "non-decreasing at {}", i as f64 * 0.01);
            prev = v;
        }
        // Asymmetric: the two halves compress differently.
        let (up, down) = (t.eval(2.0), -t.eval(-2.0));
        assert!(
            (up - down).abs() / up.max(down) > 0.15,
            "lopsided: +2 -> {up:.2}, -2 -> {down:.2}"
        );
        assert!(
            t.eval(9.0) == t.eval(4.0) && t.eval(-9.0) == t.eval(-4.0),
            "held flat beyond the table"
        );
        // The table agrees with the model it was built from.
        for u in [-3.7, -1.23, -0.4, 0.05, 0.9, 2.2, 3.9] {
            let direct = -triode_plate_voltage(grid_voltage(u));
            let mid = -triode_plate_voltage(grid_voltage(0.0));
            let raw_slope = {
                let h = 1e-3;
                (-triode_plate_voltage(grid_voltage(h)) + triode_plate_voltage(grid_voltage(-h)))
                    / (2.0 * h)
            };
            let want = (direct - mid) / raw_slope;
            assert!(
                (t.eval(u) - want).abs() < 2e-3 * want.abs().max(1.0),
                "table vs model at {u}"
            );
        }
    }

    #[test]
    fn every_tube_has_a_sane_operating_point() {
        for t in ALL_TUBES {
            let sp = t.spec();
            let q = sp.quiescent();
            assert!(
                q.vgk < 0.0 && q.ip > 0.0 && q.vp > 0.0,
                "{t:?}: bias {:.1} V, {:.2} mA at {:.0} V",
                q.vgk,
                q.ip * 1000.0,
                q.vp
            );
            let check = sp.k.ip(q.vgk, q.vp);
            assert!(
                (check - q.ip).abs() / q.ip < 0.01,
                "{t:?}: the bias reproduces the stated current ({:.2} vs {:.2} mA)",
                check * 1000.0,
                q.ip * 1000.0
            );
            // Plate voltage swings the right way and stays on the supply side of the load line.
            let (hi, lo) = (
                sp.plate_voltage(&q, q.vgk - 3.0 * q.vgk.abs()),
                sp.plate_voltage(&q, 0.0),
            );
            assert!(
                hi > q.vp && q.vp > lo && lo >= 0.0,
                "{t:?}: plate {lo:.0} < {:.0} < {hi:.0} V",
                q.vp
            );
        }
        // Published power-triode biases: 300B about -62 V at 300 V / 65 mA; 2A3 about -45 V at 250 V / 60 mA.
        assert!(
            (-66.0..-56.0).contains(&Tube::T300b.bias()),
            "300B bias {:.1} V",
            Tube::T300b.bias()
        );
        assert!(
            (-50.0..-40.0).contains(&Tube::A2a3.bias()),
            "2A3 bias {:.1} V",
            Tube::A2a3.bias()
        );
        // EL84 single-ended class A (Mullard): 250 V, screen 250 V, 48 mA gives about -7.3 V.
        assert!(
            (Tube::El84.bias() + 7.3).abs() < 1.0,
            "EL84 bias {:.1} V",
            Tube::El84.bias()
        );
    }
}
