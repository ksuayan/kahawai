//! Anti-aliasing: the plan for each sample rate, the Kaiser low-pass, and the
//! per-channel oversampling state.

use super::shaper::Shaper;

/// FIR taps per polyphase branch. Latency is `TAPS_PER_PHASE` base-rate frames.
pub(super) const TAPS_PER_PHASE: usize = 32;

/// How aliasing is kept out of the audible band for a sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntiAlias {
    /// Oversampling factor (1 = none).
    pub factor: usize,
    /// First-order antiderivative antialiasing on the curve.
    pub adaa: bool,
}

/// The plan for a sample rate, chosen from the measurements in
/// docs/v1/Analog-Emulation.md (section 12).
pub fn anti_alias_plan(sample_rate: u32) -> AntiAlias {
    match sample_rate {
        0..=50_000 => AntiAlias {
            factor: 4,
            adaa: true,
        },
        50_001..=100_000 => AntiAlias {
            factor: 2,
            adaa: true,
        },
        _ => AntiAlias {
            factor: 1,
            adaa: true,
        },
    }
}

/// Oversampling factor for a sample rate.
pub fn oversample_factor(sample_rate: u32) -> usize {
    anti_alias_plan(sample_rate).factor
}

/// Kaiser-windowed low-pass, unity DC gain, cutoff as a fraction of the high rate.
pub(super) fn kaiser_lowpass(taps: usize, cutoff: f64) -> Vec<f32> {
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
            let sinc = if k == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * pi * cutoff * k).sin() / (pi * k)
            };
            let w =
                i0(beta * (1.0 - (2.0 * n as f64 / m - 1.0).powi(2)).max(0.0).sqrt()) / i0(beta);
            sinc * w
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| (v / sum) as f32).collect()
}

/// A doubled ring buffer: the newest `len` values are always one slice.
pub(super) struct Ring {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
}

impl Ring {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len * 2],
            len,
            pos: 0,
        }
    }
    pub(super) fn push(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.buf[self.pos + self.len] = v;
        self.pos = (self.pos + 1) % self.len;
    }
    /// Value pushed `age` pushes ago (0 = newest).
    #[inline]
    pub(super) fn at(&self, age: usize) -> f32 {
        self.buf[self.pos + self.len - 1 - age]
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
    }
}

/// Per-channel state.
pub(super) struct Chan {
    /// Recent input samples (for the interpolator).
    pub(super) xin: Ring,
    /// Recent shaped high-rate samples (for the decimator).
    pub(super) yhr: Ring,
    /// Dry path delay, matching the oversampler's latency.
    pub(super) dry: Ring,
    pub(super) dc_x1: f32,
    pub(super) dc_y1: f32,
    /// ADAA memory: previous curve input and its antiderivative.
    pub(super) adaa: (f64, f64),
    /// Sag envelope and the transformer's low-pass state.
    pub(super) env: f32,
    pub(super) lf: f32,
}

impl Chan {
    pub(super) fn new(
        taps_phase: usize,
        taps_total: usize,
        latency: usize,
        shaper: &Shaper,
    ) -> Self {
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
    pub(super) fn clear(&mut self, shaper: &Shaper) {
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
