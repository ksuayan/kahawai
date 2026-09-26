//! Tiny sample-rate converter: cubic (Catmull-Rom) interpolation.
//!
//! Why not `rubato`? The only v1 consumers are the *lossy* encode paths
//! (Opus wants 48 kHz, MP3 wants ≤48 kHz): resampling feeds a perceptual
//! encoder, where a windowed-sinc polyphase bank buys nothing audible over
//! cubic interpolation but costs a heavy dependency and tuning surface.
//! The lossless FLAC path never resamples — it encodes at the source rate.
//! If a future lossless resampling need appears (it hasn't), reach for
//! rubato then and justify it with measurements.
//!
//! [`CubicResampler`] is the streaming core used by the transcode pipeline;
//! [`resample_interleaved`] is the whole-buffer convenience wrapper. Both
//! share the exact same tap logic.

/// Streaming cubic (Catmull-Rom) resampler over interleaved f32.
///
/// Push input blocks with [`CubicResampler::push`], pull output frames with
/// [`CubicResampler::pull`], then [`CubicResampler::flush`] at end of stream.
/// State is O(1): a few frames of input history plus the position.
pub struct CubicResampler {
    channels: usize,
    from_rate: u32,
    to_rate: u32,
    /// Input frames per output frame (`from_rate / to_rate`).
    step: f64,
    /// Input-frame coordinate of the next output frame.
    pos: f64,
    /// Buffered input frames (interleaved); `window[0]` is frame `base`.
    window: Vec<f32>,
    base: u64,
    total_in: u64,
    emitted: u64,
    flushing: bool,
}

impl CubicResampler {
    /// Used by the feature-gated Opus/MP3 encoder paths; dead in the
    /// default build.
    #[allow(dead_code)]
    pub fn new(channels: usize, from_rate: u32, to_rate: u32) -> Self {
        assert!((1..=8).contains(&channels), "channel count {channels}");
        assert!(from_rate > 0 && to_rate > 0);
        Self {
            channels,
            from_rate,
            to_rate,
            step: from_rate as f64 / to_rate as f64,
            pos: 0.0,
            window: Vec::new(),
            base: 0,
            total_in: 0,
            emitted: 0,
            flushing: false,
        }
    }

    /// Feed interleaved input frames.
    pub fn push(&mut self, input: &[f32]) {
        assert!(input.len().is_multiple_of(self.channels));
        self.window.extend_from_slice(input);
        self.total_in += (input.len() / self.channels) as u64;
    }

    /// Pull up to `out.len()` interleaved output samples; returns the count
    /// written. During normal streaming only emits frames whose four
    /// Catmull-Rom taps are all real input (no edge invention mid-stream).
    /// After [`CubicResampler::flush`], emits up to the rounded total with
    /// edge-clamped taps.
    pub fn pull(&mut self, out: &mut [f32]) -> usize {
        assert!(out.len().is_multiple_of(self.channels));
        let target = self.flush_target();
        let mut written = 0;
        while written + self.channels <= out.len() {
            if self.window.is_empty() {
                break;
            }
            let i = self.pos.floor() as u64;
            // Need taps i-1..=i+2. Mid-stream all four must be buffered.
            if !self.flushing && i + 2 >= self.base + self.window_frames() {
                break;
            }
            if self.flushing && self.emitted >= target {
                break;
            }
            let t = (self.pos - i as f64) as f32;
            for c in 0..self.channels {
                out[written + c] = self.tap(i as i64 - 1, c) * cr0(t)
                    + self.tap(i as i64, c) * cr1(t)
                    + self.tap(i as i64 + 1, c) * cr2(t)
                    + self.tap(i as i64 + 2, c) * cr3(t);
            }
            written += self.channels;
            self.emitted += 1;
            self.pos += self.step;
            self.drop_old();
        }
        written
    }

    /// Begin end-of-stream: remaining output is emitted with edge-clamped
    /// taps, up to the rounded total frame count.
    pub fn flush(&mut self) {
        self.flushing = true;
    }

    fn window_frames(&self) -> u64 {
        (self.window.len() / self.channels) as u64
    }

    /// Rounded total output frames for the input seen so far. Only
    /// meaningful after all input is pushed (i.e. during flush).
    fn flush_target(&self) -> u64 {
        (self.total_in * self.to_rate as u64 + self.from_rate as u64 / 2) / self.from_rate as u64
    }

    /// Read input frame `f` (clamped to the valid range), channel `c`.
    /// During flushing, reads past the end clamp to the last frame — the
    /// same edge rule as the whole-buffer version.
    fn tap(&self, f: i64, c: usize) -> f32 {
        let last = self.base + self.window_frames() - 1;
        let f = (f.max(self.base as i64) as u64).min(last) - self.base;
        self.window[f as usize * self.channels + c]
    }

    /// Drop input frames that no future output can reference: anything with
    /// index < floor(pos) - 1 (the earliest tap is i-1).
    fn drop_old(&mut self) {
        let keep_from = self.pos.floor() as u64 as i64 - 1;
        if keep_from <= self.base as i64 {
            return;
        }
        let drop_frames = (keep_from as u64 - self.base).min(self.window_frames());
        if drop_frames > 0 {
            self.window.drain(..drop_frames as usize * self.channels);
            self.base += drop_frames;
        }
    }
}

// Catmull-Rom basis weights for t in [0,1).
#[inline]
fn cr0(t: f32) -> f32 {
    -0.5 * t + t * t - 0.5 * t * t * t
}
#[inline]
fn cr1(t: f32) -> f32 {
    1.0 - 2.5 * t * t + 1.5 * t * t * t
}
#[inline]
fn cr2(t: f32) -> f32 {
    0.5 * t + 2.0 * t * t - 1.5 * t * t * t
}
#[inline]
fn cr3(t: f32) -> f32 {
    -0.5 * t * t + 0.5 * t * t * t
}

/// Resample `input` (interleaved, `channels`) from `from_rate` to `to_rate`.
///
/// Returns the resampled interleaved buffer. If the rates are equal the
/// input is returned unchanged (cloned).
///
/// Convenience wrapper over [`CubicResampler`]; exercised by unit tests.
#[allow(dead_code)]
pub fn resample_interleaved(
    input: &[f32],
    channels: usize,
    from_rate: u32,
    to_rate: u32,
) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let in_frames = input.len() / channels;
    let out_frames = ((in_frames as u64 * to_rate as u64 + from_rate as u64 / 2) / from_rate as u64)
        .max(1) as usize;
    let mut r = CubicResampler::new(channels, from_rate, to_rate);
    r.push(input);
    r.flush();
    let mut out = vec![0.0f32; out_frames * channels];
    let mut filled = 0;
    while filled < out.len() {
        let n = r.pull(&mut out[filled..]);
        if n == 0 {
            break;
        }
        filled += n;
    }
    out.truncate(filled);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine(frames: usize, channels: usize, freq: f32, rate: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|f| {
                let v = (2.0 * PI * freq * f as f32 / rate).sin();
                vec![v; channels]
            })
            .collect()
    }

    /// Drive a resampler the way the transcode pipeline does: small input
    /// pushes, pulling after each, then flush.
    fn streaming(x: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
        let mut r = CubicResampler::new(channels, from, to);
        let mut out = Vec::new();
        let mut tmp = vec![0.0f32; 512 * channels];
        for chunk in x.chunks(300 * channels) {
            r.push(chunk);
            loop {
                let n = r.pull(&mut tmp);
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&tmp[..n]);
            }
        }
        r.flush();
        loop {
            let n = r.pull(&mut tmp);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&tmp[..n]);
        }
        out
    }

    #[test]
    fn identity_rate_returns_input() {
        let x = sine(100, 2, 440.0, 44100.0);
        let y = resample_interleaved(&x, 2, 44100, 44100);
        assert_eq!(x, y);
    }

    #[test]
    fn upsample_preserves_tone() {
        let x = sine(4410, 1, 440.0, 44100.0);
        let y = resample_interleaved(&x, 1, 44100, 48000);
        assert_eq!(y.len(), 4800);
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!((peak(&y) - peak(&x)).abs() < 0.01);
    }

    #[test]
    fn downsample_2x_halves_length() {
        let x = sine(4800, 2, 440.0, 48000.0);
        let y = resample_interleaved(&x, 2, 48000, 24000);
        assert_eq!(y.len(), 2400 * 2);
    }

    #[test]
    fn dc_stays_dc() {
        let x = vec![0.5f32; 1000 * 2];
        let y = resample_interleaved(&x, 2, 44100, 48000);
        for v in y {
            assert!((v - 0.5).abs() < 1e-6, "v={v}");
        }
    }

    #[test]
    fn channels_stay_independent() {
        let x: Vec<f32> = (0..1000).flat_map(|_| [0.25f32, -0.25f32]).collect();
        let y = resample_interleaved(&x, 2, 44100, 48000);
        for (i, v) in y.iter().enumerate() {
            let want = if i % 2 == 0 { 0.25 } else { -0.25 };
            assert!((v - want).abs() < 1e-6, "i={i} v={v}");
        }
    }

    #[test]
    fn first_frame_clamps_to_first_input_frame() {
        // Regression: the old whole-buffer code wrapped i-1 to usize::MAX at
        // i=0 and clamped to the LAST frame. A ramp must start at ~0.
        let x: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let y = resample_interleaved(&x, 1, 44100, 48000);
        assert!(y[0].abs() < 0.01, "y[0]={}", y[0]);
        assert!(
            (y[y.len() - 1] - 1.0).abs() < 0.02,
            "last={}",
            y[y.len() - 1]
        );
    }

    #[test]
    fn streaming_matches_whole_buffer() {
        for (from, to) in [
            (44100, 48000),
            (48000, 44100),
            (88200, 48000),
            (44100, 22050),
        ] {
            let x = sine(5000, 2, 440.0, from as f32);
            let a = resample_interleaved(&x, 2, from, to);
            let b = streaming(&x, 2, from, to);
            assert_eq!(a.len(), b.len(), "len {from}->{to}");
            for (i, (va, vb)) in a.iter().zip(b.iter()).enumerate() {
                assert!((va - vb).abs() < 1e-5, "{from}->{to} i={i}: {va} vs {vb}");
            }
        }
    }
}
