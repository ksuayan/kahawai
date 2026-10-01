//! Click-free gain changes for the transport and the volume.
//!
//! Cutting or starting audio mid-waveform is a step in the signal, and a step
//! is a click, loudest on bass. Two small, pure helpers fix that:
//!
//! - [`Fader`] runs **on the device side**, in the output callback, where it
//!   can shape audio that is already queued (about 200 ms is). It fades out for
//!   pause, stop, seek and skip, fades back in on resume and at the start of an
//!   interrupted stream, and softens an underrun instead of cutting to silence.
//!   It is per *frame*, so every channel gets the same gain.
//! - [`AmpRamp`] runs in the engine for the **volume**, which used to jump once
//!   per decoded chunk (about 93 ms), a zipper you hear while dragging the
//!   slider. It ramps per frame instead.
//!
//! Neither touches audio at unity gain, bit for bit, and neither is used on the
//! exclusive (bit-perfect, DoP) paths, where fading would change the samples.

/// Device-side fade for pause, stop, seek, skip, resume and underruns.
#[derive(Debug, Clone)]
pub struct Fader {
    gain: f32,
    target: f32,
    /// Gain change per frame while ramping.
    step: f32,
    ramp_frames: u32,
}

impl Fader {
    /// A fader whose full fade takes `ramp_frames` frames (about 8 ms is
    /// inaudible as a fade and long enough to avoid a click), starting at
    /// `initial_gain` (0.0 to fade a new stream in, 1.0 to leave it untouched).
    pub fn new(ramp_frames: u32, initial_gain: f32) -> Self {
        let ramp_frames = ramp_frames.max(1);
        Self {
            gain: initial_gain.clamp(0.0, 1.0),
            target: 1.0,
            step: 1.0 / ramp_frames as f32,
            ramp_frames,
        }
    }

    /// Fade to silence (`false`) or back to full (`true`) from wherever it is.
    pub fn set_audible(&mut self, audible: bool) {
        self.target = if audible { 1.0 } else { 0.0 };
    }

    /// Fully faded out and staying there: nothing needs playing.
    pub fn is_silent(&self) -> bool {
        self.gain <= 0.0 && self.target <= 0.0
    }

    /// At full gain and staying there: audio passes through unchanged.
    fn is_unity(&self) -> bool {
        self.gain >= 1.0 && self.target >= 1.0
    }

    /// Current gain, 0.0 to 1.0.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Apply the fade to interleaved `data`.
    pub fn process(&mut self, data: &mut [f32], channels: usize) {
        if self.is_unity() {
            return; // bit-exact pass-through
        }
        if self.is_silent() {
            data.fill(0.0);
            return;
        }
        for frame in data.chunks_mut(channels.max(1)) {
            if self.gain < self.target {
                self.gain = (self.gain + self.step).min(self.target);
            } else if self.gain > self.target {
                self.gain = (self.gain - self.step).max(self.target);
            }
            for s in frame {
                *s *= self.gain;
            }
        }
    }

    /// The source ran dry: only `filled` samples of `data` are real audio.
    /// Fade out the end of what there is rather than cutting it off, silence
    /// the rest, and mute until audio returns (the next [`process`](Self::process)
    /// then fades back in from zero).
    pub fn underrun(&mut self, data: &mut [f32], filled: usize, channels: usize) {
        let channels = channels.max(1);
        let filled = filled.min(data.len());
        self.process(&mut data[..filled], channels);
        let frames = filled / channels;
        let tail = frames.min(self.ramp_frames as usize);
        for i in 0..tail {
            // 1.0 at the start of the tail, falling to 0.0 at its end.
            let m = 1.0 - (i + 1) as f32 / tail as f32;
            let base = (frames - tail + i) * channels;
            for s in &mut data[base..base + channels] {
                *s *= m;
            }
        }
        data[filled..].fill(0.0);
        self.gain = 0.0;
    }
}

/// Engine-side volume ramp: moves the volume to its target over a few
/// milliseconds, per frame, so a change never lands as a step.
#[derive(Debug, Clone)]
pub struct AmpRamp {
    current: f32,
    target: f32,
    step: f32,
}

impl AmpRamp {
    /// A ramp that spans a full 0.0 to 1.0 change in `ramp_frames` frames.
    pub fn new(ramp_frames: u32) -> Self {
        Self {
            current: 1.0,
            target: 1.0,
            step: 1.0 / ramp_frames.max(1) as f32,
        }
    }

    /// Re-time the ramp (a new sample rate) without moving the volume.
    pub fn set_ramp_frames(&mut self, ramp_frames: u32) {
        self.step = 1.0 / ramp_frames.max(1) as f32;
    }

    /// Jump to `v` at once (the start of a stream: nothing to smooth).
    pub fn snap(&mut self, v: f32) {
        self.current = v;
        self.target = v;
    }

    /// Ramp to `v`.
    pub fn set_target(&mut self, v: f32) {
        self.target = v;
    }

    /// Whether the volume is exactly 1.0 and staying there.
    pub fn is_unity(&self) -> bool {
        self.current == 1.0 && self.target == 1.0
    }

    /// Apply to interleaved `data`, `channels` samples per frame.
    pub fn apply(&mut self, data: &mut [f32], channels: usize) {
        if self.is_unity() {
            return;
        }
        for frame in data.chunks_mut(channels.max(1)) {
            if self.current < self.target {
                self.current = (self.current + self.step).min(self.target);
            } else if self.current > self.target {
                self.current = (self.current - self.step).max(self.target);
            }
            for s in frame {
                *s *= self.current;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;
    const RAMP: u32 = 384; // 8 ms at 48 kHz

    /// Interleaved sine, `freq` Hz at `amp`, starting at frame `from`.
    fn sine(freq: f32, amp: f32, from: usize, frames: usize, channels: usize) -> Vec<f32> {
        (from..from + frames)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / RATE).sin();
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    /// Largest jump between consecutive frames (left channel).
    fn max_step(x: &[f32], channels: usize) -> f32 {
        let m: Vec<f32> = x.iter().step_by(channels).copied().collect();
        m.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    /// The biggest step a clean sine of this frequency and amplitude has.
    fn natural_step(freq: f32, amp: f32) -> f32 {
        2.0 * std::f32::consts::PI * freq / RATE * amp
    }

    // -- Fader ---------------------------------------------------------------

    #[test]
    fn at_unity_the_fader_is_bit_transparent() {
        let mut f = Fader::new(RAMP, 1.0);
        let input = sine(220.0, 0.7, 0, 2000, 2);
        let mut out = input.clone();
        f.process(&mut out, 2);
        assert_eq!(out, input);
    }

    #[test]
    fn fading_out_reaches_silence_smoothly_and_stays_there() {
        let mut f = Fader::new(RAMP, 1.0);
        f.set_audible(false);
        let mut out = vec![1.0f32; (RAMP as usize + 100) * 2];
        f.process(&mut out, 2);
        for w in out.chunks(2).collect::<Vec<_>>().windows(2) {
            assert!(w[1][0] <= w[0][0] + 1e-6, "monotonic fall");
            assert!(w[0][0] - w[1][0] <= 1.0 / RAMP as f32 + 1e-6, "no big step");
        }
        assert_eq!(out[out.len() - 1], 0.0);
        assert!(f.is_silent());
        let mut more = vec![0.5f32; 64];
        f.process(&mut more, 2);
        assert!(
            more.iter().all(|&s| s == 0.0),
            "silence: the ring is not consumed or played"
        );
    }

    #[test]
    fn a_pause_at_the_worst_moment_has_no_click() {
        // A hard cut at a sine peak is a jump of nearly the whole amplitude.
        let (freq, amp) = (60.0, 0.8);
        let quarter = (RATE / freq / 4.0) as usize; // the first peak
        let before = sine(freq, amp, 0, quarter, 2);
        let mut f = Fader::new(RAMP, 1.0);
        let mut played = before.clone();
        f.process(&mut played, 2);
        f.set_audible(false);
        let mut rest = sine(freq, amp, quarter, 2000, 2);
        f.process(&mut rest, 2);
        played.extend(rest);
        assert!(
            max_step(&played, 2) <= natural_step(freq, amp) * 1.05 + 1.0 / RAMP as f32 * amp,
            "stepped {} (a cut would step ~{amp})",
            max_step(&played, 2)
        );
        // and without the fader it is a click:
        let mut cut = sine(freq, amp, 0, quarter, 2);
        cut.extend(vec![0.0f32; 400]);
        assert!(max_step(&cut, 2) > amp * 0.9);
    }

    #[test]
    fn resuming_fades_in_from_silence_without_a_click() {
        let (freq, amp) = (80.0, 0.8);
        let mut f = Fader::new(RAMP, 1.0);
        f.set_audible(false);
        let mut gone = vec![0.3f32; 2 * RAMP as usize + 10];
        f.process(&mut gone, 2);
        assert!(f.is_silent());
        f.set_audible(true);
        // Resume at the audio's peak, the worst place to start.
        let mut audio = sine(freq, amp, (RATE / freq / 4.0) as usize, 1500, 2);
        f.process(&mut audio, 2);
        assert!(
            audio[0].abs() < amp * 0.01,
            "starts from silence: {}",
            audio[0]
        );
        assert!(max_step(&audio, 2) <= natural_step(freq, amp) * 1.05 + 1.0 / RAMP as f32 * amp);
        assert_eq!(f.gain(), 1.0);
        let mut after = sine(freq, amp, 5000, 100, 2);
        let want = after.clone();
        f.process(&mut after, 2);
        assert_eq!(after, want, "back to untouched");
    }

    #[test]
    fn a_new_stream_can_start_muted_and_fade_in_or_start_untouched() {
        let mut quiet = Fader::new(RAMP, 0.0);
        let mut a = vec![1.0f32; 2 * RAMP as usize];
        quiet.process(&mut a, 2);
        assert!(a[0] < 0.01 && a[a.len() - 1] > 0.99);
        let mut loud = Fader::new(RAMP, 1.0);
        let mut b = vec![1.0f32; 100];
        loud.process(&mut b, 2);
        assert!(
            b.iter().all(|&s| s == 1.0),
            "a clean gapless start is not touched"
        );
    }

    #[test]
    fn an_underrun_fades_out_what_is_there_instead_of_cutting_it() {
        let (freq, amp) = (60.0, 0.8);
        let mut f = Fader::new(RAMP, 1.0);
        // A callback asks for 2048 frames but only 1000 exist, ending at a peak.
        let mut data = vec![9.9f32; 2048 * 2];
        let real = sine(freq, amp, 0, 1000, 2);
        data[..real.len()].copy_from_slice(&real);
        f.underrun(&mut data, real.len(), 2);
        assert!(data[real.len()..].iter().all(|&s| s == 0.0));
        assert!(max_step(&data, 2) <= natural_step(freq, amp) * 1.05 + 1.0 / RAMP as f32 * amp);
        assert!(
            data[(1000 - 1) * 2].abs() < 0.01,
            "the last real frame is faded to ~0"
        );
        assert_eq!(f.gain(), 0.0, "muted until audio returns");
        // Audio returns: it fades in from zero.
        let mut back = sine(freq, amp, 3000, 1500, 2);
        f.process(&mut back, 2);
        assert!(back[0].abs() < 0.01);
        assert!(max_step(&back, 2) <= natural_step(freq, amp) * 1.05 + 1.0 / RAMP as f32 * amp);
    }

    #[test]
    fn every_channel_gets_the_same_gain_per_frame() {
        let mut f = Fader::new(RAMP, 0.0);
        let mut data = vec![1.0f32; 6 * 100]; // 6 channels
        f.process(&mut data, 6);
        for frame in data.chunks(6) {
            assert!(frame.iter().all(|&s| s == frame[0]), "{frame:?}");
        }
    }

    // -- AmpRamp ---------------------------------------------------------------

    #[test]
    fn at_unity_the_volume_ramp_is_bit_transparent() {
        let mut r = AmpRamp::new(480);
        let input = sine(330.0, 0.6, 0, 1000, 2);
        let mut out = input.clone();
        r.apply(&mut out, 2);
        assert_eq!(out, input);
    }

    #[test]
    fn a_volume_change_ramps_per_frame_not_in_one_step() {
        // Dropping to 20 %: a chunk-wide multiply steps by 0.8 of the signal at
        // once; the ramp spreads it over 10 ms.
        let (freq, amp) = (100.0, 0.9);
        let mut r = AmpRamp::new(480);
        r.set_target(0.2);
        let mut out = sine(freq, amp, 0, 4000, 2);
        let clean = out.clone();
        r.apply(&mut out, 2);
        assert!(max_step(&out, 2) <= natural_step(freq, amp) * 1.1 + amp / 480.0);
        // a whole-chunk multiply, for comparison, is one big step:
        let mut stepped = clean[..2000].to_vec();
        stepped.extend(clean[2000..].iter().map(|s| s * 0.2));
        assert!(
            max_step(&stepped, 2) > amp * 0.3,
            "the old behaviour clicks"
        );
        let last = out.chunks(2).last().unwrap()[0];
        let want = clean.chunks(2).last().unwrap()[0] * 0.2;
        assert!((last - want).abs() < 1e-5, "settles at the target");
        assert!(!r.is_unity());
    }

    #[test]
    fn ramping_is_per_frame_so_stereo_ramps_as_fast_as_mono() {
        let mut mono = AmpRamp::new(100);
        let mut stereo = AmpRamp::new(100);
        mono.set_target(0.0);
        stereo.set_target(0.0);
        let mut m = vec![1.0f32; 100];
        let mut s = vec![1.0f32; 200];
        mono.apply(&mut m, 1);
        stereo.apply(&mut s, 2);
        assert!(
            m[99].abs() < 1e-5 && s[198].abs() < 1e-5,
            "both done in 100 frames"
        );
    }

    #[test]
    fn a_new_target_mid_ramp_continues_from_where_it_is() {
        let mut r = AmpRamp::new(100);
        r.set_target(0.0);
        let mut a = vec![1.0f32; 50];
        r.apply(&mut a, 1);
        let mid = a[49];
        r.set_target(1.0);
        let mut b = vec![1.0f32; 10];
        r.apply(&mut b, 1);
        assert!(
            b[0] > mid,
            "turns around from the current value, no jump: {} -> {}",
            mid,
            b[0]
        );
        assert!(b[0] - mid < 0.02);
    }

    #[test]
    fn snap_sets_the_volume_with_no_ramp() {
        let mut r = AmpRamp::new(480);
        r.snap(0.5);
        let mut d = vec![1.0f32; 10];
        r.apply(&mut d, 1);
        assert!(d.iter().all(|&s| s == 0.5));
    }
}
