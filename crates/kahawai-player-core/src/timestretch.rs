//! Pitch-preserving time stretch (WSOLA) for spoken-word playback.
//!
//! Audiobooks are heard faster or slower than they were recorded. Resampling
//! would raise or lower the voice with the speed, so this stage changes the
//! duration without touching the pitch: it cuts the signal into overlapping
//! 25 ms frames, moves their start points apart (faster) or together
//! (slower) than they were, and before each frame is laid down it nudges it a
//! few milliseconds to the spot where it best continues the previous one
//! (waveform-similarity overlap-add, WSOLA). Speech-grade, not music-grade:
//! it is for voices.
//!
//! Unlike the in-place [`DspStage`](crate::dsp::DspStage)s, a stretch
//! changes the number of frames, so it has its own `process` that appends to
//! an output buffer, and it sits first in the chain, right after decode.
//! At a rate of exactly 1.0 it is a bypass and the samples are bit-identical.
//! Leaving a stretch for 1.0 hands the audio over without a seam.

/// Slowest and fastest rate the stage accepts (the UI offers 0.75 to 2.5).
pub const RATE_RANGE: (f32, f32) = (0.5, 3.0);

/// Frame length, seconds. Long enough to hold a couple of pitch periods of a
/// low male voice, short enough not to smear consonants.
const FRAME_SECONDS: f32 = 0.025;
/// How far, seconds, a frame may be nudged to find its best fit.
const SEARCH_SECONDS: f32 = 0.010;

pub struct TimeStretch {
    channels: usize,
    /// Frame length N (even) and synthesis hop H = N / 2, in frames.
    n: usize,
    h: usize,
    /// Search range either side of the nominal start, in frames.
    delta: usize,
    rate: f32,
    /// Periodic Hann window of length N: `w[i] + w[i + H] == 1`.
    window: Vec<f32>,
    /// Interleaved input not yet fully consumed, and the absolute frame
    /// index (since the last reset) of its first frame.
    inp: Vec<f32>,
    /// The same input mixed to mono, for the similarity search.
    mono: Vec<f32>,
    base: i64,
    /// The faded-out half of the last frame, still to be overlapped (H frames).
    tail: Vec<f32>,
    /// Absolute start of the last frame laid down.
    prev_start: i64,
    /// Where the next frame would start if nothing moved it.
    nominal: f64,
    /// A stretch is running (state above is meaningful).
    active: bool,
    /// Frames of the original signal heard so far (the media position).
    media: u64,
    /// The first frame's tail has been taken from the input.
    primed_tail: bool,
}

impl TimeStretch {
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let mut s = Self {
            channels: channels.max(1),
            n: 2,
            h: 1,
            delta: 1,
            rate: 1.0,
            window: Vec::new(),
            inp: Vec::new(),
            mono: Vec::new(),
            base: 0,
            tail: Vec::new(),
            prev_start: 0,
            nominal: 0.0,
            active: false,
            media: 0,
            primed_tail: false,
        };
        s.prepare(sample_rate, channels);
        s
    }

    /// Set the sample rate and channel count the audio will have. Drops any
    /// state: call it when a stream opens.
    pub fn prepare(&mut self, sample_rate: u32, channels: usize) {
        let rate = sample_rate.max(8000) as f32;
        self.channels = channels.max(1);
        let n = ((rate * FRAME_SECONDS) as usize).max(64) & !1;
        self.n = n;
        self.h = n / 2;
        self.delta = ((rate * SEARCH_SECONDS) as usize).max(8);
        self.window = (0..n)
            .map(|i| {
                let x = std::f32::consts::PI * i as f32 / n as f32;
                x.sin() * x.sin()
            })
            .collect();
        self.reset();
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// Change the rate (clamped to [`RATE_RANGE`]; NaN means 1.0). Takes
    /// effect from the next frame, with no seam.
    pub fn set_rate(&mut self, rate: f32) {
        self.rate = if rate.is_finite() {
            rate.clamp(RATE_RANGE.0, RATE_RANGE.1)
        } else {
            1.0
        };
    }

    /// Forget everything: a new track, or a seek.
    pub fn reset(&mut self) {
        self.inp.clear();
        self.mono.clear();
        self.tail.clear();
        self.base = 0;
        self.prev_start = 0;
        self.nominal = 0.0;
        self.active = false;
        self.media = 0;
        self.primed_tail = false;
    }

    /// Frames of the original signal heard so far since the last reset. At
    /// rate r this runs r times faster than the output frames produced.
    pub fn media_frames(&self) -> u64 {
        self.media
    }

    /// True when the stage will pass audio through untouched.
    pub fn is_bypassing(&self) -> bool {
        !self.active && self.rate == 1.0
    }

    /// Process `input` (interleaved), appending what is ready to `out`.
    /// Output is delayed by about a frame while the first one fills.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        if self.rate == 1.0 {
            if self.active {
                self.hand_over(input, out);
            } else {
                out.extend_from_slice(input);
                self.media += (input.len() / ch) as u64;
            }
            return;
        }
        self.push(input);
        if !self.active {
            self.start();
        }
        self.run(out);
    }

    /// Is some input still held back, waiting for its frame to fill?
    pub fn has_pending(&self) -> bool {
        self.active && (self.base + self.mono.len() as i64) as u64 > self.media
    }

    /// The input has ended: append the audio still held back. The held
    /// input is padded with silence just long enough to complete the last
    /// frame, and the result is cut to the length the held media should
    /// take, so the padding is never heard. Leaves the stage idle.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if !self.has_pending() {
            return;
        }
        let end = self.base + self.mono.len() as i64;
        let remaining = (end - (self.prev_start + self.h as i64)).max(0) as f64;
        let want = (remaining / self.rate as f64).ceil() as usize * self.channels;
        let from = out.len();
        let pad = vec![0.0f32; (self.n + self.delta + self.n) * self.channels];
        self.push(&pad);
        self.run(out);
        out.truncate((from + want).min(out.len()));
        self.media = end as u64;
        self.inp.clear();
        self.mono.clear();
        self.tail.clear();
        self.active = false;
        self.base = self.media as i64;
    }

    fn push(&mut self, input: &[f32]) {
        let ch = self.channels;
        let frames = input.len() / ch;
        self.inp.extend_from_slice(&input[..frames * ch]);
        for f in input.chunks_exact(ch) {
            self.mono.push(f.iter().sum::<f32>() / ch as f32);
        }
    }

    /// Begin a stretch at the current end of the media: primed so the first
    /// block out is the input itself, with no fade-in dip.
    fn start(&mut self) {
        // Continue from where bypassing (or a reset) left the media.
        self.base = self.media as i64;
        self.prev_start = self.base - self.h as i64;
        self.nominal = self.base as f64;
        self.tail.clear();
        self.tail.resize(self.h * self.channels, 0.0);
        self.active = true;
        self.primed_tail = false;
    }

    fn at(&self, abs: i64) -> usize {
        (abs - self.base) as usize
    }

    /// Lay down as many frames as the buffered input allows.
    fn run(&mut self, out: &mut Vec<f32>) {
        let (n, h, ch) = (self.n as i64, self.h as i64, self.channels);
        let delta = self.delta as i64;
        loop {
            let end = self.base + (self.mono.len() as i64);
            let nominal = self.nominal.round() as i64;
            let hi = nominal + delta;
            // Wait until the last candidate frame, and the tail target, fit.
            if hi + n > end || self.prev_start + n > end {
                break;
            }
            if !self.primed_tail {
                // First frame: tail = faded second half of x[s0 - H + ..]: the
                // virtual previous frame's tail is x[s0 .. s0 + H] windowed.
                let s0 = self.base;
                let o = self.at(s0);
                for i in 0..self.h {
                    for c in 0..ch {
                        self.tail[i * ch + c] =
                            self.window[self.h + i] * self.inp[(o + i) * ch + c];
                    }
                }
                self.primed_tail = true;
            }
            let lo = (nominal - delta).max(self.base);
            let s = self.best_start(lo, hi);
            let o = self.at(s);
            // Overlap-add: previous tail + this frame's rising half.
            for i in 0..self.h {
                for c in 0..ch {
                    out.push(self.tail[i * ch + c] + self.window[i] * self.inp[(o + i) * ch + c]);
                }
            }
            for i in 0..self.h {
                for c in 0..ch {
                    self.tail[i * ch + c] =
                        self.window[self.h + i] * self.inp[(o + self.h + i) * ch + c];
                }
            }
            self.prev_start = s;
            self.media = (s + h) as u64;
            self.nominal += self.h as f64 * self.rate as f64;
        }
        self.compact();
    }

    /// The start in `lo..=hi` whose first H frames best continue the last
    /// frame's natural continuation (normalised cross-correlation on mono).
    fn best_start(&self, lo: i64, hi: i64) -> i64 {
        let h = self.h;
        let target_at = self.at(self.prev_start + self.h as i64);
        let target = &self.mono[target_at..target_at + h];
        let nominal = self.nominal.round() as i64;
        let mut best = (f32::NEG_INFINITY, nominal.clamp(lo, hi));
        // Energy of the candidate window, slid along.
        let first = self.at(lo);
        let mut energy: f32 = self.mono[first..first + h].iter().map(|x| x * x).sum();
        for s in lo..=hi {
            let a = self.at(s);
            let cand = &self.mono[a..a + h];
            let dot: f32 = target.iter().zip(cand).map(|(x, y)| x * y).sum();
            // Normalised, with a small pull toward the nominal start so a
            // silent or flat stretch does not wander.
            let score = dot / (energy.max(1e-9)).sqrt() - 1e-6 * (s - nominal).abs() as f32;
            if score > best.0 {
                best = (score, s);
            }
            if s < hi {
                let leaving = self.mono[a];
                let entering = self.mono[a + h];
                energy += entering * entering - leaving * leaving;
            }
        }
        best.1
    }

    /// Drop input nothing can need any more.
    fn compact(&mut self) {
        let keep_from = (self.prev_start + self.h as i64)
            .min(self.nominal.round() as i64 - self.delta as i64)
            .max(self.base);
        let drop = (keep_from - self.base) as usize;
        if drop >= 4096 {
            self.inp.drain(..drop * self.channels);
            self.mono.drain(..drop);
            self.base += drop as i64;
        }
    }

    /// Leaving a stretch for 1.0: continue the media from where the last
    /// frame's natural continuation lies, raw. Because the window halves sum
    /// to one, dropping the faded tail and playing the original from there
    /// is exactly what the next overlap-add would have produced.
    fn hand_over(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.push(input);
        let from = self
            .at(self.prev_start + self.h as i64)
            .min(self.mono.len());
        out.extend_from_slice(&self.inp[from * self.channels..]);
        self.media = (self.base + self.mono.len() as i64) as u64;
        self.inp.clear();
        self.mono.clear();
        self.tail.clear();
        self.active = false;
        self.base = self.media as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: u32 = 44100;

    fn sine(freq: f32, frames: usize, channels: usize, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / FS as f32).sin();
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    /// Voice-like: a harmonic series on f0 with a slow amplitude wobble.
    fn voice(f0: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / FS as f32;
                let env = 0.6 + 0.4 * (2.0 * std::f32::consts::PI * 3.0 * t).sin();
                let mut v = 0.0;
                for k in 1..=8 {
                    v += (2.0 * std::f32::consts::PI * f0 * k as f32 * t).sin() / k as f32;
                }
                0.3 * env * v
            })
            .collect()
    }

    fn run(stage: &mut TimeStretch, input: &[f32], chunk: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for c in input.chunks(chunk * stage.channels) {
            stage.process(c, &mut out);
        }
        out
    }

    /// Frequency by counting rising zero crossings over `frames` of mono.
    fn freq_of(mono: &[f32]) -> f32 {
        let mut crossings = Vec::new();
        for i in 1..mono.len() {
            if mono[i - 1] < 0.0 && mono[i] >= 0.0 {
                // linear interpolation for the exact crossing
                let frac = -mono[i - 1] / (mono[i] - mono[i - 1]);
                crossings.push(i as f32 - 1.0 + frac);
            }
        }
        if crossings.len() < 3 {
            return 0.0;
        }
        let periods = (crossings.len() - 1) as f32;
        FS as f32 * periods / (crossings.last().unwrap() - crossings[0])
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn rate_one_is_a_bit_identical_bypass() {
        let input = voice(130.0, 20_000);
        let mut st = TimeStretch::new(1, FS);
        let out = run(&mut st, &input, 777);
        assert_eq!(out, input);
        assert!(st.is_bypassing());
        assert_eq!(st.media_frames(), input.len() as u64);
    }

    #[test]
    fn the_rate_is_clamped_and_nan_means_one() {
        let mut st = TimeStretch::new(2, FS);
        st.set_rate(10.0);
        assert_eq!(st.rate(), RATE_RANGE.1);
        st.set_rate(0.01);
        assert_eq!(st.rate(), RATE_RANGE.0);
        st.set_rate(f32::NAN);
        assert_eq!(st.rate(), 1.0);
    }

    #[test]
    fn output_length_follows_the_rate() {
        let input = sine(220.0, FS as usize * 6, 1, 0.5);
        for rate in [0.5f32, 0.75, 1.25, 1.5, 2.0, 2.5, 3.0] {
            let mut st = TimeStretch::new(1, FS);
            st.set_rate(rate);
            let out = run(&mut st, &input, 4096);
            let want = input.len() as f32 / rate;
            let slack = (st.n + st.delta) as f32 / rate + st.h as f32;
            assert!(
                (out.len() as f32 - want).abs() < slack + 0.01 * want,
                "rate {rate}: {} frames, wanted about {want}",
                out.len()
            );
        }
    }

    #[test]
    fn pitch_is_preserved_when_speeding_up_and_slowing_down() {
        let input = sine(440.0, FS as usize * 4, 1, 0.5);
        for rate in [0.6f32, 0.75, 1.5, 2.0, 2.5, 3.0] {
            let mut st = TimeStretch::new(1, FS);
            st.set_rate(rate);
            let out = run(&mut st, &input, 2048);
            let mid = &out[out.len() / 4..out.len() * 3 / 4];
            let f = freq_of(mid);
            assert!(
                (f - 440.0).abs() < 440.0 * 0.01,
                "rate {rate}: a 440 Hz tone came out at {f} Hz"
            );
            // The level holds too: overlap-add must not dip or swell.
            let r = rms(mid);
            assert!((r - 0.5 / 2f32.sqrt()).abs() < 0.03, "rate {rate}: rms {r}");
        }
    }

    #[test]
    fn a_voice_keeps_its_fundamental_at_double_speed() {
        // Estimate f0 by autocorrelation over a window of the output.
        let f0 = 120.0;
        let input = voice(f0, FS as usize * 5);
        let mut st = TimeStretch::new(1, FS);
        st.set_rate(2.0);
        let out = run(&mut st, &input, 3000);
        let w = &out[FS as usize / 2..FS as usize / 2 + 8192];
        let (mut best, mut best_lag) = (f32::MIN, 0);
        for lag in (FS as usize / 400)..(FS as usize / 70) {
            let c: f32 = w[..w.len() - lag]
                .iter()
                .zip(&w[lag..])
                .map(|(a, b)| a * b)
                .sum();
            if c > best {
                best = c;
                best_lag = lag;
            }
        }
        let est = FS as f32 / best_lag as f32;
        assert!((est - f0).abs() < 4.0, "f0 {est} Hz, expected {f0}");
    }

    #[test]
    fn the_output_does_not_depend_on_how_the_input_was_chunked() {
        let input = voice(150.0, FS as usize * 3);
        let mut a = TimeStretch::new(1, FS);
        a.set_rate(1.7);
        let whole = run(&mut a, &input, input.len());
        for chunk in [1usize, 97, 1024, 5000] {
            let mut b = TimeStretch::new(1, FS);
            b.set_rate(1.7);
            let parts = run(&mut b, &input, chunk);
            assert_eq!(parts, whole, "chunk {chunk}");
        }
    }

    #[test]
    fn channels_stay_in_step() {
        // Left a 300 Hz tone, right a 600 Hz tone: each keeps its pitch and
        // they remain locked together (same frame count).
        let frames = FS as usize * 3;
        let mut input = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / FS as f32;
            input.push(0.4 * (2.0 * std::f32::consts::PI * 300.0 * t).sin());
            input.push(0.4 * (2.0 * std::f32::consts::PI * 600.0 * t).sin());
        }
        let mut st = TimeStretch::new(2, FS);
        st.set_rate(1.5);
        let out = run(&mut st, &input, 4096);
        assert_eq!(out.len() % 2, 0);
        let l: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let r: Vec<f32> = out.chunks_exact(2).map(|f| f[1]).collect();
        let (lm, rm) = (
            &l[l.len() / 4..l.len() * 3 / 4],
            &r[r.len() / 4..r.len() * 3 / 4],
        );
        assert!((freq_of(lm) - 300.0).abs() < 3.0, "left {}", freq_of(lm));
        assert!((freq_of(rm) - 600.0).abs() < 6.0, "right {}", freq_of(rm));
    }

    #[test]
    fn media_position_runs_at_the_rate() {
        let input = sine(330.0, FS as usize * 6, 1, 0.5);
        for rate in [0.75f32, 1.5, 2.5] {
            let mut st = TimeStretch::new(1, FS);
            st.set_rate(rate);
            let out = run(&mut st, &input, 4096);
            let expect = out.len() as f64 * rate as f64;
            let got = st.media_frames() as f64;
            assert!(
                (got - expect).abs() < st.h as f64 * rate as f64 + 2.0,
                "rate {rate}: media {got}, output {} x rate = {expect}",
                out.len()
            );
        }
    }

    #[test]
    fn a_rate_change_mid_stream_does_not_step_the_signal() {
        let input = sine(200.0, FS as usize * 6, 1, 0.5);
        let mut st = TimeStretch::new(1, FS);
        let mut out = Vec::new();
        for (i, c) in input.chunks(2048).enumerate() {
            st.set_rate([1.0, 1.4, 2.0, 0.8, 1.2][(i / 25) % 5]);
            st.process(c, &mut out);
        }
        // A clean 200 Hz sine at 0.5 never moves more than this per frame.
        let natural = 0.5 * 2.0 * std::f32::consts::PI * 200.0 / FS as f32;
        let step = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            step < natural * 1.6,
            "largest step {step}, a clean tone steps {natural}"
        );
    }

    #[test]
    fn leaving_a_stretch_for_normal_speed_hands_over_without_a_seam() {
        let input = sine(250.0, FS as usize * 4, 1, 0.5);
        let mut st = TimeStretch::new(1, FS);
        st.set_rate(2.0);
        let mut out = Vec::new();
        let split = FS as usize * 2;
        for c in input[..split].chunks(2048) {
            st.process(c, &mut out);
        }
        let stretched = out.len();
        st.set_rate(1.0);
        for c in input[split..].chunks(2048) {
            st.process(c, &mut out);
        }
        assert!(st.is_bypassing());
        // After the hand-over the audio is the original, sample for sample,
        // through to the end of the input.
        let tail_len = FS as usize;
        assert_eq!(
            &out[out.len() - tail_len..],
            &input[input.len() - tail_len..]
        );
        // And there is no jump where it joins.
        let natural = 0.5 * 2.0 * std::f32::consts::PI * 250.0 / FS as f32;
        let around = &out[stretched.saturating_sub(64)..(stretched + 4096).min(out.len())];
        let step = around
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            step < natural * 1.6,
            "join stepped {step}, a clean tone steps {natural}"
        );
        assert_eq!(
            st.media_frames(),
            input.len() as u64,
            "every media frame was heard"
        );
    }

    #[test]
    fn flushing_at_the_end_plays_the_held_back_tail_and_no_padding() {
        let input = sine(330.0, FS as usize * 3 + 1234, 2, 0.5);
        for rate in [0.75f32, 1.0, 1.5, 2.5] {
            let mut st = TimeStretch::new(2, FS);
            st.set_rate(rate);
            let mut out = run(&mut st, &input, 3000);
            st.flush(&mut out);
            let frames_in = input.len() / 2;
            let want = frames_in as f64 / rate as f64;
            assert!(
                // Frames can land up to a search range either side of nominal.
                (out.len() as f64 / 2.0 - want).abs() <= (st.delta + st.h) as f64 / rate as f64,
                "rate {rate}: {} frames out, wanted {want}",
                out.len() / 2
            );
            assert_eq!(st.media_frames(), frames_in as u64, "all media heard");
            assert!(!st.has_pending());
            // The last frames are the tone, not the zero padding.
            let last = &out[out.len() - 400..];
            assert!(rms(last) > 0.2, "rate {rate}: tail rms {}", rms(last));
        }
    }

    #[test]
    fn reset_forgets_the_past() {
        let mut st = TimeStretch::new(1, FS);
        st.set_rate(1.5);
        let loud = sine(500.0, FS as usize, 1, 0.9);
        let mut out = Vec::new();
        st.process(&loud, &mut out);
        st.reset();
        assert_eq!(st.media_frames(), 0);
        out.clear();
        let quiet = sine(300.0, FS as usize, 1, 0.1);
        st.process(&quiet, &mut out);
        assert!(
            out.iter().all(|v| v.abs() <= 0.1 + 1e-4),
            "nothing of the old signal leaks"
        );
    }

    #[test]
    fn silence_and_tiny_inputs_are_safe() {
        let mut st = TimeStretch::new(2, FS);
        st.set_rate(2.5);
        let mut out = Vec::new();
        st.process(&[], &mut out);
        st.process(&[0.1, 0.1], &mut out);
        st.process(&vec![0.0; 40_000], &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
        // An odd sample count (a torn frame) is tolerated, not panicked on.
        st.process(&[0.0; 7], &mut out);
    }
}
