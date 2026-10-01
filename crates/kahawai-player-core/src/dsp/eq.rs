//! Parametric EQ: band types and validation, filter design, and the live,
//! click-free [`ParametricEq`] stage. (Spec: kahawai-player-design.md §5.)

use kahawai_core::MusicError;
use serde::{Deserialize, Serialize};

use super::biquad::{biquad_step, Biquad, BiquadState};
use super::DspStage;

/// The largest boost (dB, never below 0) the band set applies at any
/// frequency: the worst case for headroom. Sampled on a log grid.
pub fn max_boost_db(bands: &[EqBand], sample_rate: u32) -> f32 {
    if bands.is_empty() || sample_rate == 0 {
        return 0.0;
    }
    let designed: Vec<Biquad> = bands.iter().map(|b| design_band(b, sample_rate)).collect();
    let top = (sample_rate as f64 * NYQUIST_FRACTION as f64).max(21.0);
    let mut worst = 1.0f64;
    for i in 0..512 {
        let f = 20.0 * (top / 20.0).powf(i as f64 / 511.0);
        let w = 2.0 * std::f64::consts::PI * f / sample_rate as f64;
        let mag: f64 = designed.iter().map(|b| b.magnitude(w)).product();
        worst = worst.max(mag);
    }
    (20.0 * worst.log10()) as f32
}

/// EQ band type. Wire shape is snake_case for the Tauri bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBandType {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

/// One EQ band. `q` is the RBJ Q for peaking/low-pass/high-pass; for
/// shelves it is the shelf slope S (clamped to 0.1..=3.0).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub band_type: EqBandType,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Maximum simultaneous bands. Headphone-correction profiles (AutoEq) are
/// usually ten filters, so this leaves room for those plus a couple of the
/// user's own.
pub const MAX_EQ_BANDS: usize = 12;

/// Range of the EQ preamp, in dB. Correction profiles only ever cut (to leave
/// headroom for their boosts); the top end allows a deliberate makeup gain.
pub const EQ_PREAMP_RANGE_DB: (f32, f32) = (-24.0, 12.0);

fn validate_band(b: &EqBand) -> Result<(), MusicError> {
    if !(10.0..=24_000.0).contains(&b.freq) || !b.freq.is_finite() {
        return Err(MusicError::BadRequest(format!(
            "EQ band frequency out of range: {}",
            b.freq
        )));
    }
    if !(0.1..=18.0).contains(&b.q) || !b.q.is_finite() {
        return Err(MusicError::BadRequest(format!(
            "EQ band Q out of range: {}",
            b.q
        )));
    }
    if !(-24.0..=24.0).contains(&b.gain_db) || !b.gain_db.is_finite() {
        return Err(MusicError::BadRequest(format!(
            "EQ band gain out of range: {}",
            b.gain_db
        )));
    }
    Ok(())
}

/// Validate a band list without touching the live chain (used by the Tauri
/// command so the UI gets an error before anything is sent to the engine).
pub fn validate_bands(bands: &[EqBand]) -> Result<(), MusicError> {
    if bands.len() > MAX_EQ_BANDS {
        return Err(MusicError::BadRequest(format!(
            "at most {MAX_EQ_BANDS} EQ bands"
        )));
    }
    for b in bands {
        validate_band(b)?;
    }
    Ok(())
}

/// Highest band frequency that stays meaningful at `sample_rate`: a fraction
/// of Nyquist, since the RBJ formulas degenerate as w0 approaches pi.
pub const NYQUIST_FRACTION: f32 = 0.45;

/// The frequency a band is actually designed at (its own, capped for the rate).
pub fn usable_freq(freq: f32, sample_rate: u32) -> f32 {
    freq.min(sample_rate as f32 * NYQUIST_FRACTION)
}

/// Design one band at `sample_rate` using the RBJ cookbook.
pub(super) fn design_band(band: &EqBand, sample_rate: u32) -> Biquad {
    let a = 10f64.powf(band.gain_db as f64 / 40.0);
    let freq = usable_freq(band.freq, sample_rate) as f64;
    let q = band.q as f64;
    let w0 = 2.0 * std::f64::consts::PI * freq / sample_rate as f64;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);

    let (b0, b1, b2, a0, a1, a2) = match band.band_type {
        EqBandType::Peaking => (
            1.0 + alpha * a,
            -2.0 * cw,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cw,
            1.0 - alpha / a,
        ),
        EqBandType::LowShelf => {
            let s = q.clamp(0.1, 3.0);
            let alpha_s = sw / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
            let sq = 2.0 * a.sqrt() * alpha_s;
            (
                a * ((a + 1.0) - (a - 1.0) * cw + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                a * ((a + 1.0) - (a - 1.0) * cw - sq),
                (a + 1.0) + (a - 1.0) * cw + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                (a + 1.0) + (a - 1.0) * cw - sq,
            )
        }
        EqBandType::HighShelf => {
            let s = q.clamp(0.1, 3.0);
            let alpha_s = sw / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
            let sq = 2.0 * a.sqrt() * alpha_s;
            (
                a * ((a + 1.0) + (a - 1.0) * cw + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                a * ((a + 1.0) + (a - 1.0) * cw - sq),
                (a + 1.0) - (a - 1.0) * cw + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * cw),
                (a + 1.0) - (a - 1.0) * cw - sq,
            )
        }
        EqBandType::LowPass => {
            let c = (1.0 - cw) / 2.0;
            (c, 1.0 - cw, c, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
        }
        EqBandType::HighPass => {
            let c = (1.0 + cw) / 2.0;
            (c, -(1.0 + cw), c, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
        }
    };
    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

impl DspStage for ParametricEq {
    fn prepare(&mut self, sample_rate: u32) {
        self.set_sample_rate(sample_rate);
    }
    fn process(&mut self, interleaved: &mut [f32], channels: usize) {
        ParametricEq::process(self, interleaved, channels);
    }
    fn reset(&mut self) {
        let rate = self.sample_rate;
        let bands = self.bands.clone();
        self.slots.clear();
        self.primed = false;
        self.mix = if self.enabled { 1.0 } else { 0.0 };
        let designed = self.design_all(&bands);
        self.slots = designed.into_iter().map(Slot::snapped).collect();
        self.sample_rate = rate;
    }
}

/// How long a live change (bands, on/off) takes to fade in, in seconds.
/// Long enough to be free of clicks, short enough to feel immediate.
const EQ_RAMP_SECONDS: f32 = 0.015;

const IDENTITY: Biquad = Biquad {
    b0: 1.0,
    b1: 0.0,
    b2: 0.0,
    a1: 0.0,
    a2: 0.0,
};

/// One filter stage. On a live edit the coefficients glide from `cur` to
/// `target` while the filter state is kept, which is what avoids clicks.
struct Slot {
    cur: Biquad,
    target: Biquad,
    step: Biquad,
    remaining: usize,
    /// Fading toward a pass-through; dropped once it gets there.
    removing: bool,
    /// One state pair per channel.
    states: Vec<BiquadState>,
}

impl Slot {
    fn snapped(c: Biquad) -> Self {
        Self {
            cur: c,
            target: c,
            step: Biquad {
                b0: 0.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            },
            remaining: 0,
            removing: false,
            states: Vec::new(),
        }
    }

    fn retarget(&mut self, to: Biquad, frames: usize, removing: bool) {
        let n = frames.max(1) as f64;
        self.step = Biquad {
            b0: (to.b0 - self.cur.b0) / n,
            b1: (to.b1 - self.cur.b1) / n,
            b2: (to.b2 - self.cur.b2) / n,
            a1: (to.a1 - self.cur.a1) / n,
            a2: (to.a2 - self.cur.a2) / n,
        };
        self.target = to;
        self.remaining = frames.max(1);
        self.removing = removing;
    }

    #[inline]
    fn advance(&mut self) {
        if self.remaining == 0 {
            return;
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.cur = self.target;
        } else {
            self.cur.b0 += self.step.b0;
            self.cur.b1 += self.step.b1;
            self.cur.b2 += self.step.b2;
            self.cur.a1 += self.step.a1;
            self.cur.a2 += self.step.a2;
        }
    }
}

/// Up-to-8-band parametric EQ over interleaved f32.
///
/// Bypass is bit-transparent: once faded out (or with no bands) `process`
/// does not touch the buffer at all. Edits made while audio is flowing (new
/// bands, on/off) fade in over ~15 ms with the filter state kept, so
/// dragging a control point does not click. Before any audio has passed
/// (a new track or rate), changes apply at once.
pub struct ParametricEq {
    bands: Vec<EqBand>,
    enabled: bool,
    sample_rate: u32,
    slots: Vec<Slot>,
    /// Wet/dry mix: 1 = fully equalized, 0 = untouched. Moves toward the
    /// enabled/disabled target so toggling does not click.
    mix: f32,
    /// Audio has passed through since the last reset; only then is it worth
    /// fading a change.
    primed: bool,
}

impl ParametricEq {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            bands: Vec::new(),
            enabled: true,
            sample_rate,
            slots: Vec::new(),
            mix: 1.0,
            primed: false,
        }
    }

    fn ramp_frames(&self) -> usize {
        ((self.sample_rate as f32 * EQ_RAMP_SECONDS) as usize).max(1)
    }

    fn design_all(&self, bands: &[EqBand]) -> Vec<Biquad> {
        bands
            .iter()
            .map(|b| design_band(b, self.sample_rate))
            .collect()
    }

    pub fn set_bands(&mut self, bands: Vec<EqBand>) -> Result<(), MusicError> {
        validate_bands(&bands)?;
        let designed = self.design_all(&bands);
        self.bands = bands;
        if !self.primed {
            self.slots = designed.into_iter().map(Slot::snapped).collect();
            return Ok(());
        }
        let ramp = self.ramp_frames();
        for (i, c) in designed.iter().enumerate() {
            match self.slots.get_mut(i) {
                Some(slot) => slot.retarget(*c, ramp, false),
                None => {
                    // A new band fades in from a pass-through.
                    let mut slot = Slot::snapped(IDENTITY);
                    slot.retarget(*c, ramp, false);
                    self.slots.push(slot);
                }
            }
        }
        for slot in self.slots.iter_mut().skip(designed.len()) {
            slot.retarget(IDENTITY, ramp, true);
        }
        Ok(())
    }

    /// Worst-case boost of the live bands (0 when the EQ is off or flat).
    pub fn max_boost_db(&self) -> f32 {
        if self.enabled {
            max_boost_db(&self.bands, self.sample_rate)
        } else {
            0.0
        }
    }

    pub fn bands(&self) -> &[EqBand] {
        &self.bands
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !self.primed {
            self.mix = if enabled { 1.0 } else { 0.0 };
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Redesign filters when the stream rate changes; state is cleared
    /// (a rate change is a track boundary, never mid-track).
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            let designed = self.design_all(&self.bands);
            self.slots = designed.into_iter().map(Slot::snapped).collect();
            self.primed = false;
            self.mix = if self.enabled { 1.0 } else { 0.0 };
        }
    }

    fn ensure_states(&mut self, channels: usize) {
        for slot in &mut self.slots {
            if slot.states.len() != channels {
                slot.states = vec![BiquadState::default(); channels];
            }
        }
    }

    /// Process interleaved samples in place. Bit-transparent no-op unless
    /// enabled with at least one band (or still fading out).
    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let target_mix = if self.enabled { 1.0 } else { 0.0 };
        if self.slots.is_empty() || (self.mix == 0.0 && target_mix == 0.0) {
            self.mix = target_mix;
            return;
        }
        debug_assert_eq!(samples.len() % channels, 0);
        self.ensure_states(channels);
        if self.mix == 0.0 {
            // Coming back from bypass: stale state would ring, start clean.
            for slot in &mut self.slots {
                slot.states
                    .iter_mut()
                    .for_each(|s| *s = BiquadState::default());
            }
        }
        self.primed = true;

        let steady = self.mix == target_mix && self.slots.iter().all(|s| s.remaining == 0);
        if steady && target_mix == 1.0 {
            // Fast path: nothing is changing.
            for slot in self.slots.iter_mut() {
                let c = slot.cur;
                for frame in samples.chunks_exact_mut(channels) {
                    for (smp, st) in frame.iter_mut().zip(slot.states.iter_mut()) {
                        *smp = biquad_step(&c, st, *smp);
                    }
                }
            }
            return;
        }

        let mix_step = 1.0 / self.ramp_frames() as f32;
        let mut wet = vec![0.0f32; channels];
        for frame in samples.chunks_exact_mut(channels) {
            if self.mix != target_mix {
                self.mix = if target_mix > self.mix {
                    (self.mix + mix_step).min(target_mix)
                } else {
                    (self.mix - mix_step).max(target_mix)
                };
            }
            for slot in &mut self.slots {
                slot.advance();
            }
            for (ch, w) in wet.iter_mut().enumerate() {
                let mut v = frame[ch];
                for slot in &mut self.slots {
                    v = biquad_step(&slot.cur, &mut slot.states[ch], v);
                }
                *w = v;
            }
            let m = self.mix;
            for (smp, w) in frame.iter_mut().zip(wet.iter()) {
                *smp = if m >= 1.0 { *w } else { *smp + (*w - *smp) * m };
            }
        }
        // Bands that finished fading to a pass-through are done.
        self.slots.retain(|s| !(s.removing && s.remaining == 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::test_util::{sine, stereo};

    fn rms_steady(samples: &[f32], skip: usize) -> f32 {
        let tail = &samples[skip.min(samples.len())..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len().max(1) as f32).sqrt()
    }

    #[test]
    fn bypass_is_bit_transparent() {
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }])
        .unwrap();
        eq.set_enabled(false);
        let input = stereo(&sine(440.0, 8192, 44100, 0.5));
        let mut out = input.clone();
        eq.process(&mut out, 2);
        assert_eq!(out, input, "disabled EQ must not touch a single sample");
    }

    #[test]
    fn empty_chain_is_transparent() {
        let mut eq = ParametricEq::new(48000);
        let input = stereo(&sine(440.0, 4096, 48000, 0.5));
        let mut out = input.clone();
        eq.process(&mut out, 2);
        assert_eq!(out, input);
    }

    #[test]
    fn low_frequency_bands_stay_accurate_at_192k() {
        // A 100 Hz low shelf at 192 kHz needs coefficients ~1e-3 from 1.0;
        // f32 coefficients lose accuracy here. +12 dB well below the corner must be x3.98.
        let mut eq = ParametricEq::new(192_000);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::LowShelf,
            freq: 100.0,
            gain_db: 12.0,
            q: 0.7,
        }])
        .unwrap();
        let mut out = stereo(&sine(15.0, 192_000 * 4, 192_000, 0.1));
        eq.process(&mut out, 2);
        let ratio = rms_steady(&out, 192_000 * 2 * 2) / (0.1 * std::f32::consts::FRAC_1_SQRT_2);
        assert!(
            (ratio - 3.98).abs() < 0.12,
            "expected ~x3.98 at 15 Hz, got {ratio}"
        );
    }

    #[test]
    fn band_above_the_usable_range_is_designed_at_the_cap_and_stays_stable() {
        let rate = 44_100;
        let cap = usable_freq(24_000.0, rate);
        assert!(cap < 22_050.0 * 0.95, "capped below Nyquist, got {cap}");
        let band = |f| EqBand {
            band_type: EqBandType::HighShelf,
            freq: f,
            gain_db: 6.0,
            q: 0.7,
        };
        let input = stereo(&sine(5000.0, 8192, rate, 0.5));
        let (mut a, mut b) = (input.clone(), input);
        let mut eq_hi = ParametricEq::new(rate);
        eq_hi.set_bands(vec![band(24_000.0)]).unwrap();
        eq_hi.process(&mut a, 2);
        let mut eq_cap = ParametricEq::new(rate);
        eq_cap.set_bands(vec![band(cap)]).unwrap();
        eq_cap.process(&mut b, 2);
        assert!(a.iter().all(|s| s.is_finite() && s.abs() < 4.0));
        assert_eq!(
            a, b,
            "an out-of-range frequency behaves exactly like the cap"
        );
        assert_eq!(
            usable_freq(1000.0, rate),
            1000.0,
            "in-range bands are untouched"
        );
    }

    fn peak_band(gain_db: f32) -> EqBand {
        EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db,
            q: 1.0,
        }
    }

    /// Largest jump between neighbouring samples of a channel.
    fn max_step(interleaved: &[f32], channels: usize) -> f32 {
        interleaved
            .iter()
            .step_by(channels)
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn live_band_changes_glide_instead_of_clicking() {
        // A low shelf on a bass tone: resetting the filter state or jumping
        // the coefficients here throws the waveform by a large fraction of
        // its amplitude, while the tone itself moves only ~0.01 per sample.
        let rate = 44100;
        let shelf = |gain_db| EqBand {
            band_type: EqBandType::LowShelf,
            freq: 150.0,
            gain_db,
            q: 0.7,
        };
        let mut eq = ParametricEq::new(rate);
        eq.set_bands(vec![shelf(2.0)]).unwrap();
        let tone = stereo(&sine(60.0, rate as usize * 2, rate, 0.3));
        let split = (rate as usize / 2 + 184) * 2; // near a waveform peak
        let (first, second) = tone.split_at(split);
        let mut a = first.to_vec();
        eq.process(&mut a, 2); // audio is flowing now
        eq.set_bands(vec![shelf(12.0)]).unwrap(); // drag: +2 dB -> +12 dB
        let mut b = second.to_vec();
        eq.process(&mut b, 2);
        // Measure across the seam, where the change happened.
        let all = [a.clone(), b.clone()].concat();
        let step = max_step(&all[2000..], 2);
        assert!(
            step < 0.02,
            "no click while the band changes, biggest step {step}"
        );
        // ...and it lands where a filter that was +12 dB all along would.
        let mut reference = ParametricEq::new(rate);
        reference.set_bands(vec![shelf(12.0)]).unwrap();
        let mut steady = tone.clone();
        reference.process(&mut steady, 2);
        let tail = rms_steady(&b, b.len() - 4410 * 2);
        let want = rms_steady(&steady, steady.len() - 4410 * 2);
        assert!(
            (tail - want).abs() / want < 0.02,
            "settles at +12 dB: {tail} vs {want}"
        );
    }

    #[test]
    fn adding_and_removing_bands_live_fades_without_a_click() {
        let rate = 44100;
        let mut eq = ParametricEq::new(rate);
        let tone = stereo(&sine(1000.0, rate as usize * 2, rate, 0.3));
        let chunk = tone.len() / 4;
        let mut out = tone[..chunk].to_vec();
        eq.process(&mut out, 2); // flat, flowing
        eq.set_bands(vec![peak_band(6.0)]).unwrap(); // add
        let mut added = tone[chunk..chunk * 2].to_vec();
        eq.process(&mut added, 2);
        assert!(
            max_step(&[out.clone(), added.clone()].concat(), 2) < 0.1,
            "adding a band is smooth"
        );
        assert!(
            rms_steady(&added, added.len() - 4410) > 0.3 * 1.8 * 0.70,
            "the added band is audible"
        );
        eq.set_bands(vec![]).unwrap(); // remove
        let mut removed = tone[chunk * 2..chunk * 3].to_vec();
        let src = removed.clone();
        eq.process(&mut removed, 2);
        assert!(
            max_step(&[added.clone(), removed.clone()].concat(), 2) < 0.1,
            "removing a band is smooth"
        );
        let n = removed.len() - 4410;
        let err = removed[n..]
            .iter()
            .zip(&src[n..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 1e-3, "back to the untouched signal, error {err}");
        let mut again = tone[chunk * 3..].to_vec();
        let src = again.clone();
        eq.process(&mut again, 2);
        assert_eq!(again, src, "an empty chain is bit-transparent again");
    }

    #[test]
    fn toggling_the_eq_live_fades_and_then_is_bit_transparent() {
        let rate = 44100;
        let mut eq = ParametricEq::new(rate);
        eq.set_bands(vec![peak_band(12.0)]).unwrap();
        let tone = stereo(&sine(1000.0, rate as usize * 2, rate, 0.3));
        let chunk = tone.len() / 4;
        let mut warm = tone[..chunk].to_vec();
        eq.process(&mut warm, 2);
        eq.set_enabled(false);
        let mut fade = tone[chunk..chunk * 2].to_vec();
        eq.process(&mut fade, 2);
        assert!(
            max_step(&[warm.clone(), fade.clone()].concat(), 2) < 0.2,
            "switching off is smooth"
        );
        let n = fade.len() - 2000;
        assert_eq!(
            &fade[n..],
            &tone[chunk..chunk * 2][n..],
            "faded all the way to the dry signal"
        );
        let mut off = tone[chunk * 2..chunk * 3].to_vec();
        let src = off.clone();
        eq.process(&mut off, 2);
        assert_eq!(off, src, "bit-transparent once faded out");
        // Switching back on fades in, without ringing from stale state.
        eq.set_enabled(true);
        let mut on = tone[chunk * 3..].to_vec();
        eq.process(&mut on, 2);
        assert!(
            max_step(&[off.clone(), on.clone()].concat(), 2) < 0.2,
            "switching on is smooth"
        );
    }

    #[test]
    fn changes_before_any_audio_apply_at_once() {
        // A new track (or rate) has nothing playing, so there is nothing to fade.
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![peak_band(12.0)]).unwrap();
        eq.set_enabled(false);
        let input = stereo(&sine(1000.0, 2048, 44100, 0.3));
        let mut out = input.clone();
        eq.process(&mut out, 2);
        assert_eq!(
            out, input,
            "disabled before the first sample: untouched from sample one"
        );
    }

    #[test]
    fn peaking_boosts_in_band_only() {
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }])
        .unwrap();

        // In-band: 1 kHz should roughly double in amplitude (+6 dB).
        let mut in_band = stereo(&sine(1000.0, 44100, 44100, 0.4));
        eq.process(&mut in_band, 2);
        let ratio = rms_steady(&in_band, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (1.9..2.1).contains(&ratio),
            "expected ~2x amplitude at 1 kHz, got {ratio}"
        );

        // Out of band: 100 Hz must be (nearly) untouched by a 1 kHz Q=1 bell.
        let mut eq2 = ParametricEq::new(44100);
        eq2.set_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }])
        .unwrap();
        let mut low = stereo(&sine(100.0, 44100, 44100, 0.4));
        eq2.process(&mut low, 2);
        let low_ratio = rms_steady(&low, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (0.97..1.03).contains(&low_ratio),
            "100 Hz should be untouched, got {low_ratio}"
        );
    }

    #[test]
    fn low_shelf_boosts_lows_not_highs() {
        let mut eq = ParametricEq::new(44100);
        eq.set_bands(vec![EqBand {
            band_type: EqBandType::LowShelf,
            freq: 200.0,
            gain_db: 6.0,
            q: 0.7,
        }])
        .unwrap();
        let mut low = stereo(&sine(50.0, 44100, 44100, 0.4));
        eq.process(&mut low, 2);
        let low_ratio = rms_steady(&low, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (1.9..2.1).contains(&low_ratio),
            "50 Hz shelf boost: {low_ratio}"
        );

        let mut eq2 = ParametricEq::new(44100);
        eq2.set_bands(vec![EqBand {
            band_type: EqBandType::LowShelf,
            freq: 200.0,
            gain_db: 6.0,
            q: 0.7,
        }])
        .unwrap();
        let mut high = stereo(&sine(8000.0, 44100, 44100, 0.4));
        eq2.process(&mut high, 2);
        let high_ratio = rms_steady(&high, 22050) / 0.4 / std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (0.97..1.03).contains(&high_ratio),
            "8 kHz untouched: {high_ratio}"
        );
    }

    #[test]
    fn band_validation_rejects_garbage() {
        assert!(validate_bands(
            &[EqBand {
                band_type: EqBandType::Peaking,
                freq: 1000.0,
                gain_db: 0.0,
                q: 1.0,
            }; MAX_EQ_BANDS + 1]
        )
        .is_err());
        assert!(validate_bands(&[EqBand {
            band_type: EqBandType::Peaking,
            freq: 5.0,
            gain_db: 0.0,
            q: 1.0,
        }])
        .is_err());
        assert!(validate_bands(&[EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 0.0,
            q: 0.0,
        }])
        .is_err());
        assert!(validate_bands(&[EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }])
        .is_ok());
    }

    #[test]
    fn max_boost_of_no_bands_or_cuts_is_zero() {
        assert_eq!(max_boost_db(&[], 44_100), 0.0);
        let cut = EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: -6.0,
            q: 1.0,
        };
        assert_eq!(
            max_boost_db(&[cut], 44_100),
            0.0,
            "a cut never raises the peak"
        );
    }

    #[test]
    fn max_boost_finds_the_peak_of_a_band() {
        let b = EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        };
        let m = max_boost_db(&[b], 44_100);
        assert!(
            (m - 6.0).abs() < 0.2,
            "a +6 dB peaking band boosts about 6 dB, got {m}"
        );
    }

    #[test]
    fn max_boost_of_several_bands_is_the_true_worst_case_not_their_sum() {
        // The reported EQ: low shelf +2.5, peaks +1.5 and +1.5, high shelf +2.
        let bands = [
            EqBand {
                band_type: EqBandType::LowShelf,
                freq: 100.0,
                gain_db: 2.5,
                q: 0.7,
            },
            EqBand {
                band_type: EqBandType::Peaking,
                freq: 250.0,
                gain_db: 1.5,
                q: 1.0,
            },
            EqBand {
                band_type: EqBandType::Peaking,
                freq: 3000.0,
                gain_db: 1.5,
                q: 1.0,
            },
            EqBand {
                band_type: EqBandType::HighShelf,
                freq: 10_000.0,
                gain_db: 2.0,
                q: 0.7,
            },
        ];
        let m = max_boost_db(&bands, 44_100);
        assert!(
            m > 2.0 && m < 4.0,
            "somewhere near the strongest band, well under the sum of all four: {m}"
        );
    }
}
