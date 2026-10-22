//! Biquad filters (RBJ "Audio EQ Cookbook"): normalized coefficients and
//! the per-channel filter state. Shared by the EQ and the loudness meter's
//! K-weighting.

/// Normalized biquad coefficients (a0 = 1). f64: at 176/192 kHz a low-
/// frequency band needs coefficients within ~1e-6 of 1.0, which f32 cannot
/// represent well enough (noisy or slightly wrong response).
#[derive(Debug, Clone, Copy)]
pub(super) struct Biquad {
    pub(super) b0: f64,
    pub(super) b1: f64,
    pub(super) b2: f64,
    pub(super) a1: f64,
    pub(super) a2: f64,
}

impl Biquad {
    /// |H(e^jw)| at angular frequency `w` (a0 is 1).
    pub(super) fn magnitude(&self, w: f64) -> f64 {
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }
}

/// Direct Form II transposed state, one per channel.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct BiquadState {
    pub(super) z1: f64,
    pub(super) z2: f64,
}

/// Audio stays f32; only the filter arithmetic is f64.
#[inline]
pub(super) fn biquad_step(c: &Biquad, s: &mut BiquadState, x: f32) -> f32 {
    let x = x as f64;
    let y = c.b0 * x + s.z1;
    s.z1 = c.b1 * x - c.a1 * y + s.z2;
    s.z2 = c.b2 * x - c.a2 * y;
    y as f32
}

/// RBJ "Audio EQ Cookbook" design helpers, normalized (a0 = 1), f64.
/// Shared by the de-hiss crossover, the tape record/repro EQ and head bump.
/// `freq_hz` is capped below Nyquist the way `eq.rs` does it.
fn rbj_pre(freq_hz: f64, sample_rate: f64) -> (f64, f64) {
    let freq = freq_hz.min(sample_rate * 0.45).max(1.0);
    let w0 = 2.0 * std::f64::consts::PI * freq / sample_rate;
    (w0.cos(), w0.sin())
}

fn rbj_normalize(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Biquad {
    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// Second-order low-pass. Q = 1/√2 is Butterworth (the LR4 building block).
pub(super) fn rbj_lowpass(freq_hz: f64, q: f64, sample_rate: f64) -> Biquad {
    let (cw, sw) = rbj_pre(freq_hz, sample_rate);
    let alpha = sw / (2.0 * q);
    let c = (1.0 - cw) / 2.0;
    rbj_normalize(c, 1.0 - cw, c, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
}

/// Peaking bell, `gain_db` at `freq_hz`.
pub(super) fn rbj_peaking(freq_hz: f64, q: f64, gain_db: f64, sample_rate: f64) -> Biquad {
    let (cw, sw) = rbj_pre(freq_hz, sample_rate);
    let a = 10f64.powf(gain_db / 40.0);
    let alpha = sw / (2.0 * q);
    rbj_normalize(
        1.0 + alpha * a,
        -2.0 * cw,
        1.0 - alpha * a,
        1.0 + alpha / a,
        -2.0 * cw,
        1.0 - alpha / a,
    )
}
