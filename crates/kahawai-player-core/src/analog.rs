//! Analog character: an optional tube or transistor "warmth" stage for the
//! shared PCM path. (Plan: Analog-Emulation.md, Phase 1.)
//!
//! Signal flow per channel, all in place on interleaved f32:
//!
//! ```text
//! x -> drive -> [oversample -> asymmetric/symmetric tanh -> decimate]
//!        -> DC blocker -> output trim & gain match ─┐
//! x -> latency-matched delay ─────────────────────── mix -> out
//! ```
//!
//! It models *character* (level-dependent harmonics and soft knee), not a
//! specific circuit. Pure Rust, no platform imports. PCM only: the DoP and
//! bit-perfect paths never call it.

use serde::{Deserialize, Serialize};

use crate::dsp::DspStage;

/// Which character to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnalogFlavour {
    /// Asymmetric soft curve: mostly 2nd harmonic (even), like a triode stage.
    #[default]
    WarmTriode,
    /// Symmetric soft curve: odd harmonics only, like a transistor stage.
    SolidState,
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
// The curves
// ---------------------------------------------------------------------------

/// Largest bias of the asymmetric curve (at full drive). The bias grows with
/// the square root of drive: zero drive is a clean, linear pass-through, a
/// little drive is already lopsided (even harmonics dominate), and pushing
/// harder saturates it without losing that balance (see the harmonic table in
/// Analog-Emulation.md).
const TRIODE_BIAS_MAX: f32 = 0.8;

/// Input gain for a drive setting: 1x at 0, 8x at 1 (squared for a gentle start).
fn drive_gain(drive: f32) -> f32 {
    1.0 + 7.0 * drive * drive
}

/// A curve with its per-sample constants worked out once per frame.
#[derive(Clone, Copy)]
struct Curve {
    flavour: AnalogFlavour,
    g: f32,
    bias: f32,
    tanh_bias: f32,
    norm: f32,
}

impl Curve {
    fn new(flavour: AnalogFlavour, g: f32) -> Self {
        // Invert `drive_gain` to scale the bias with drive.
        let drive = ((g - 1.0) / 7.0).max(0.0).sqrt();
        let bias = if flavour == AnalogFlavour::WarmTriode { TRIODE_BIAS_MAX * drive.sqrt() } else { 0.0 };
        let tanh_bias = bias.tanh();
        // Unity small-signal gain: a quiet signal passes unchanged and
        // drive only adds character as the level rises.
        let norm = match flavour {
            AnalogFlavour::WarmTriode => 1.0 / (g * (1.0 - tanh_bias * tanh_bias)),
            AnalogFlavour::SolidState => 1.0 / g,
        };
        Self { flavour, g, bias, tanh_bias, norm }
    }

    #[inline]
    fn apply(&self, x: f32) -> f32 {
        match self.flavour {
            AnalogFlavour::WarmTriode => ((self.g * x + self.bias).tanh() - self.tanh_bias) * self.norm,
            AnalogFlavour::SolidState => (self.g * x).tanh() * self.norm,
        }
    }
}

/// Gain that makes the processed level match the dry level for a -12 dBFS
/// RMS sine: what "auto gain match" applies.
fn gain_match(flavour: AnalogFlavour, g: f32) -> f32 {
    const N: usize = 2048;
    let amp = 0.354_f32; // -12 dBFS RMS
    let mut sum_in = 0.0f64;
    let curve = Curve::new(flavour, g);
    let mut ys = [0.0f32; N];
    for (i, y) in ys.iter_mut().enumerate() {
        let x = amp * (2.0 * std::f32::consts::PI * (i as f32) / 64.0).sin();
        sum_in += (x as f64) * (x as f64);
        *y = curve.apply(x);
    }
    let mean = ys.iter().map(|&v| v as f64).sum::<f64>() / N as f64; // DC is removed downstream
    let sum_out: f64 = ys.iter().map(|&v| (v as f64 - mean).powi(2)).sum();
    if sum_out <= 1e-12 {
        return 1.0;
    }
    ((sum_in / sum_out).sqrt() as f32).clamp(0.25, 4.0)
}

// ---------------------------------------------------------------------------
// Oversampling filter
// ---------------------------------------------------------------------------

/// FIR taps per polyphase branch. Latency is `TAPS_PER_PHASE` base-rate frames.
const TAPS_PER_PHASE: usize = 32;

/// Oversampling factor for a sample rate: enough to keep aliasing out of the
/// audible band (measured in research/analog-spike), and none where the
/// source already has headroom.
pub fn oversample_factor(sample_rate: u32) -> usize {
    match sample_rate {
        0..=50_000 => 4,
        50_001..=100_000 => 2,
        _ => 1,
    }
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
}

impl Chan {
    fn new(taps_phase: usize, taps_total: usize, latency: usize) -> Self {
        Self {
            xin: Ring::new(taps_phase),
            yhr: Ring::new(taps_total),
            dry: Ring::new(latency.max(1)),
            dc_x1: 0.0,
            dc_y1: 0.0,
        }
    }
    fn clear(&mut self) {
        self.xin.clear();
        self.yhr.clear();
        self.dry.clear();
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
    }
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

/// How long parameter changes and on/off take to fade, in seconds.
const RAMP_SECONDS: f32 = 0.015;

pub struct AnalogStage {
    settings: AnalogSettings,
    /// The curve in use. A flavour change waits for a fade-out (see `pending`).
    active: AnalogFlavour,
    pending: Option<AnalogFlavour>,
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
        let mut s = Self {
            settings: AnalogSettings::default(),
            active: AnalogFlavour::default(),
            pending: None,
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
        self.l = oversample_factor(self.sample_rate);
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
        let comp = if s.auto_gain { gain_match(self.active, g) } else { 1.0 };
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
        if want == self.active {
            self.pending = None;
        } else if !self.primed || self.fade == 0.0 {
            self.active = want;
            self.pending = None;
        } else {
            self.pending = Some(want);
        }
        if !self.primed {
            self.snap_params();
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
            self.chans = (0..channels)
                .map(|_| Chan::new(TAPS_PER_PHASE + 1, taps_total, self.latency()))
                .collect();
        }
    }
}

impl DspStage for AnalogStage {
    fn prepare(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.design();
            self.primed = false;
            self.snap();
        }
    }

    fn latency_frames(&self) -> u32 {
        if self.settings.enabled || self.fade > 0.0 { self.latency() as u32 } else { 0 }
    }

    fn reset(&mut self) {
        self.chans.iter_mut().for_each(Chan::clear);
        self.primed = false;
        self.snap();
    }

    fn process(&mut self, samples: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        if self.fade == 0.0 {
            if let Some(p) = self.pending.take() {
                self.active = p;
            }
        }
        let mut target_fade = if self.settings.enabled && self.pending.is_none() { 1.0 } else { 0.0 };
        if self.fade == 0.0 && target_fade == 0.0 {
            return; // bit-transparent bypass
        }
        self.ensure_chans(channels);
        if self.fade == 0.0 {
            // Coming back from bypass: stale filter history would ring.
            self.chans.iter_mut().for_each(Chan::clear);
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
        let mut flavour = self.active;
        let l = self.l;
        let latency = self.latency();
        let (dc_r, taps_phase) = (self.dc_r, TAPS_PER_PHASE + 1);
        let gain_up = l as f32;

        for frame in samples.chunks_exact_mut(channels) {
            // Glide the parameters (linear, per frame).
            step(&mut self.g, tg, &mut sg);
            step(&mut self.comp, tcomp, &mut sc);
            step(&mut self.out, tout, &mut so);
            step(&mut self.mix, tmix, &mut sm);
            step(&mut self.fade, target_fade, &mut sf);
            if self.fade == 0.0 && self.pending.is_some() {
                // Faded out mid-block: swap the curve, clear its history and
                // fade back in, right here (not at the next block).
                self.active = self.pending.take().expect("checked above");
                flavour = self.active;
                self.chans.iter_mut().for_each(Chan::clear);
                self.comp = if self.settings.auto_gain { gain_match(flavour, tg) } else { 1.0 };
                tcomp = self.comp;
                sc = 0.0;
                target_fade = if self.settings.enabled { 1.0 } else { 0.0 };
                sf = (target_fade - self.fade) / ramp;
            }
            let (comp, out, mix, fade) = (self.comp, self.out, self.mix, self.fade);
            let curve = Curve::new(flavour, self.g);

            for (ch, smp) in frame.iter_mut().enumerate() {
                let x = *smp;
                let st = &mut self.chans[ch];
                let wet = if l == 1 {
                    curve.apply(x)
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
                        st.yhr.push(curve.apply(acc * gain_up));
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
        AnalogSettings { enabled: true, flavour, drive, mix, output_db: 0.0, auto_gain: false }
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
        assert!(h2(0.0, 0.05) < -60.0, "a quiet signal at zero drive stays clean");
    }

    /// Everything audible that is not a harmonic of the tone is aliasing.
    fn alias_db(fs: u32, tone_hz: f64, drive: f32, amp: f32) -> f64 {
        let n = 1 << 14;
        let bin = (tone_hz / fs as f64 * n as f64).round() as usize;
        let mut st = AnalogStage::new(fs);
        st.set_settings(on(AnalogFlavour::WarmTriode, drive, 1.0));
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

    #[test]
    fn oversampling_keeps_aliasing_below_audibility() {
        // A hard test tone (research/analog-spike: the same drive without
        // protection aliases at -14 dB at 44.1 kHz).
        let a441 = alias_db(44_100, 9_500.0, 0.53, 0.8);
        assert!(a441 < -60.0, "44.1 kHz aliasing {a441:.1} dB");
        let a48 = alias_db(48_000, 9_500.0, 0.53, 0.8);
        assert!(a48 < -60.0, "48 kHz aliasing {a48:.1} dB");
        let a96 = alias_db(96_000, 9_500.0, 0.53, 0.8);
        assert!(a96 < -75.0, "96 kHz aliasing {a96:.1} dB");
        assert_eq!((oversample_factor(44_100), oversample_factor(96_000), oversample_factor(192_000)), (4, 2, 1));
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
