//! The curves: the shaper (with ADAA) over a transfer table, drive, sag,
//! transformer colour, and the automatic gain match.

use super::models::{
    germanium_diode_table, hard_transistor_table, jfet_table, push_pull_table, silicon_diode_table,
    tube_table, Tube, TubeTable,
};
use super::settings::AnalogFlavour;

/// Input gain for a drive setting: 1x at 0, 8x at 1 (squared for a gentle start).
pub(super) fn drive_gain(drive: f32) -> f32 {
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
pub(super) struct Shaper {
    kind: ShaperKind,
}

#[derive(Clone, Copy)]
enum ShaperKind {
    Table(&'static TubeTable),
    Tanh,
    Linear,
}

impl Shaper {
    pub(super) fn new(flavour: AnalogFlavour) -> Self {
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
    pub(super) fn apply(&self, adaa: bool, st: &mut (f64, f64), u: f32) -> f32 {
        let u = u as f64;
        if !adaa {
            return self.f(u) as f32;
        }
        let a = self.anti(u);
        let du = u - st.0;
        let y = if du.abs() < 1e-6 {
            self.f(0.5 * (u + st.0))
        } else {
            (a - st.1) / du
        };
        *st = (u, a);
        y as f32
    }

    /// Initial ADAA state (input 0).
    pub(super) fn rest(&self) -> (f64, f64) {
        (0.0, self.anti(0.0))
    }
}

// Sag: an envelope of the driven level (fast attack, slower recovery) lowers
// the headroom (the stage is driven a little harder) and the output level.
pub(super) const SAG_ATTACK_S: f32 = 0.005;
pub(super) const SAG_RELEASE_S: f32 = 0.12;
/// At full sag and a saturated envelope: how much harder the curve is driven,
/// and how much quieter the stage gets.
const SAG_DRIVE: f32 = 0.5;
const SAG_LEVEL: f32 = 0.2;

/// What the sag envelope does at a level: `(extra drive, output factor)`.
#[inline]
pub(super) fn sag_effect(sag: f32, env: f32) -> (f32, f32) {
    let sagf = sag * env / (1.0 + env);
    (1.0 + SAG_DRIVE * sagf, 1.0 - SAG_LEVEL * sagf)
}

// Transformer colour: the bass (below about 90 Hz) is soft-clipped, blended
// in by the amount; everything above passes through unchanged.
pub(super) const XF_CORNER_HZ: f32 = 90.0;
pub(super) const XF_HARDNESS: f32 = 6.0;

/// Gain that makes the processed level match the dry level for a -12 dBFS
/// RMS sine: what "auto gain match" applies.
pub(super) fn gain_match(shaper: Shaper, g: f32, sag: f32) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analog::models::{triode_table, TABLE_RANGE};
    use crate::dsp::analog::settings::ALL_FLAVOURS;
    use std::f64::consts::PI;

    #[test]
    fn antiderivative_matches_the_curve() {
        let t = triode_table();
        for u in [-5.0, -3.99, -2.0, -0.31, 0.0, 0.77, 1.9, 3.5, 4.5] {
            let h = 1e-4;
            let numeric = (t.antiderivative(u + h) - t.antiderivative(u - h)) / (2.0 * h);
            assert!(
                (numeric - t.eval(u)).abs() < 1e-4,
                "dF/du = f at {u}: {numeric} vs {}",
                t.eval(u)
            );
        }
        // Continuous across a node and across the table edge.
        let e = TABLE_RANGE;
        assert!((t.antiderivative(e + 1e-9) - t.antiderivative(e - 1e-9)).abs() < 1e-6);
        // Analytic tanh antiderivative used by the solid-state curve.
        let shaper = Shaper::new(AnalogFlavour::SolidState);
        for u in [-3.0f64, -0.5, 0.0, 1.2, 6.0] {
            let h = 1e-5;
            assert!(
                ((shaper.anti(u + h) - shaper.anti(u - h)) / (2.0 * h) - u.tanh()).abs() < 1e-6
            );
        }
    }

    #[test]
    fn adaa_reproduces_the_curve_for_slow_signals_and_constants() {
        for flavour in [AnalogFlavour::WarmTriode, AnalogFlavour::SolidState] {
            let sh = Shaper::new(flavour);
            let mut st = sh.rest();
            // A constant input: ADAA must equal f(c) once the state has settled.
            let y = (0..4).map(|_| sh.apply(true, &mut st, 0.8)).last().unwrap();
            assert!(
                (y as f64 - sh.f(0.8)).abs() < 1e-6,
                "{flavour:?}: constant input"
            );
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
            assert!(
                worst < 1e-4,
                "{flavour:?}: slow signal differs from f by {worst}"
            );
        }
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
                assert!(
                    v >= prev - 1e-9,
                    "{f:?}: non-decreasing at {}",
                    i as f64 * 0.01
                );
                prev = v;
            }
        }
    }
}
