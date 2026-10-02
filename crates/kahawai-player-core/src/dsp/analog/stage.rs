//! The stage itself: [`AnalogStage`] and its [`AnalogStatus`].

use super::oversample::{anti_alias_plan, kaiser_lowpass, AntiAlias, Chan, TAPS_PER_PHASE};
use super::settings::{AnalogFlavour, AnalogSettings, AntiAliasChoice};
use super::shaper::{
    drive_gain, gain_match, sag_effect, Shaper, SAG_ATTACK_S, SAG_RELEASE_S, XF_CORNER_HZ,
    XF_HARDNESS,
};
use crate::dsp::DspStage;

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
    pub(super) active: AnalogFlavour,
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
            settings: AnalogSettings {
                antialias: AntiAliasChoice::from_plan(plan),
                ..AnalogSettings::default()
            },
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
        let comp = if s.auto_gain {
            gain_match(self.shaper, g, s.sag)
        } else {
            1.0
        };
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
        self.settings.enabled
            && self.fade >= 1.0
            && self.pending.is_none()
            && self.pending_plan.is_none()
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
    pub(super) fn latency(&self) -> usize {
        if self.l > 1 {
            TAPS_PER_PHASE
        } else {
            0
        }
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
        if self.settings.enabled || self.fade > 0.0 {
            self.latency() as u32
        } else {
            0
        }
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
        let mut target_fade = if self.settings.enabled && !waiting {
            1.0
        } else {
            0.0
        };
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
        let (att, rel) = (
            1.0 - (-1.0 / (SAG_ATTACK_S * sr)).exp(),
            1.0 - (-1.0 / (SAG_RELEASE_S * sr)).exp(),
        );
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
                self.comp = if self.settings.auto_gain {
                    gain_match(shaper, tg, tsag)
                } else {
                    1.0
                };
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
                        st.yhr
                            .push(shaper.apply(adaa, &mut st.adaa, acc * gain_up * gd) * inv_g);
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
    *cur = if (*step >= 0.0 && next >= target) || (*step < 0.0 && next <= target) {
        target
    } else {
        next
    };
}
