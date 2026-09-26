//! Client-side sample-rate conversion: cubic (Catmull-Rom) interpolation.
//!
//! The engine inserts this **only** when the sink demands a rate the
//! stream doesn't carry (`AudioSink::preferred_sample_rate`). Same
//! rationale as the server's `CubicResampler` (its lossy-encode paths):
//! for a playback monitoring path, cubic buys everything audible over a
//! windowed-sinc bank at a fraction of the code. This is a fresh,
//! smaller implementation for kahawai-player-core — the server's streaming
//! resampler stays server-side.

/// Streaming cubic resampler over interleaved f32.
///
/// Feed input with [`push`](Self::push), pull output with
/// [`pull`](Self::pull), call [`flush`](Self::flush) at end of stream.
/// Mid-stream, only frames whose four Catmull-Rom taps are all real input
/// are emitted; after `flush`, edge taps are clamped so the tail drains.
pub struct CubicResampler {
    channels: usize,
    /// Input frames per output frame (`from_rate / to_rate`).
    step: f64,
    /// Input-frame coordinate of the next output frame.
    pos: f64,
    /// Buffered input frames, interleaved; `window[0]` is frame `base`.
    window: Vec<f32>,
    base: u64,
    total_in: u64,
    emitted: u64,
    flushing: bool,
}

impl CubicResampler {
    pub fn new(channels: usize, from_rate: u32, to_rate: u32) -> Self {
        assert!((1..=8).contains(&channels), "channel count {channels}");
        assert!(from_rate > 0 && to_rate > 0, "rates must be nonzero");
        Self {
            channels,
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
        assert_eq!(input.len() % self.channels, 0, "input must be whole frames");
        self.window.extend_from_slice(input);
        self.total_in += (input.len() / self.channels) as u64;
    }

    /// Signal end of input: remaining frames drain with edge-clamped taps.
    pub fn flush(&mut self) {
        self.flushing = true;
    }

    /// Pull up to `out.len()` interleaved output samples. Returns the
    /// count written.
    pub fn pull(&mut self, out: &mut [f32]) -> usize {
        assert_eq!(out.len() % self.channels, 0, "output must be whole frames");
        let mut written = 0;
        while written + self.channels <= out.len() {
            let frame = match self.next_frame() {
                Some(f) => f,
                None => break,
            };
            out[written..written + self.channels].copy_from_slice(&frame[..self.channels]);
            written += self.channels;
        }
        // Drop input frames that can never be tapped again.
        let keep_from = (self.pos.floor() as u64).saturating_sub(1).max(self.base);
        let drop = (keep_from - self.base) as usize * self.channels;
        if drop > 0 {
            self.window.drain(..drop.min(self.window.len()));
            self.base = keep_from;
        }
        written
    }

    /// Convenience: resample a whole interleaved buffer.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.push(input);
        // Upper bound: ceil(len * to/from) + 2 frames of slop.
        let frames = input.len() / self.channels;
        let cap = (frames as f64 / self.step).ceil() as usize + 2;
        let mut out = vec![0.0; cap * self.channels];
        let n = self.pull(&mut out);
        out.truncate(n);
        out
    }

    fn window_frames(&self) -> u64 {
        (self.window.len() / self.channels) as u64
    }

    fn next_frame(&mut self) -> Option<[f32; 8]> {
        let i = self.pos.floor() as i64;
        // Taps needed: i-1 ..= i+2.
        let first = self.base as i64;
        let last = first + self.window_frames() as i64 - 1;
        let have = |idx: i64| idx >= first && idx <= last;
        if self.flushing {
            // Stop at the rounded expected total; taps clamp at the edges.
            let expect = (self.total_in as f64 / self.step).round() as u64;
            if self.emitted >= expect {
                return None;
            }
        } else if !have(i + 2) {
            // Streaming: the past tap (i-1) edge-clamps at stream start,
            // but cubic needs real present/future taps — never invent
            // audio. This leaves ~2 frames of lookahead buffered.
            return None;
        }
        let t = (self.pos - i as f64) as f32;
        let mut frame = [0.0f32; 8];
        for (c, slot) in frame.iter_mut().enumerate().take(self.channels) {
            let tap = |idx: i64| -> f32 {
                // Edge clamp when flushing; mid-stream all taps are real
                // (guarded above).
                let clamped = idx.clamp(first, last.max(first));
                let rel = (clamped - first) as usize;
                self.window
                    .get(rel * self.channels + c)
                    .copied()
                    .unwrap_or(0.0)
            };
            let p0 = tap(i - 1);
            let p1 = tap(i);
            let p2 = tap(i + 1);
            let p3 = tap(i + 2);
            *slot = catmull_rom(p0, p1, p2, p3, t);
        }
        self.pos += self.step;
        self.emitted += 1;
        Some(frame)
    }
}

fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p1
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, freq: f32, rate: f32, channels: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin() * 0.8;
                std::iter::repeat_n(s, channels)
            })
            .collect()
    }

    fn zero_crossings(samples: &[f32], channels: usize) -> usize {
        let mono: Vec<f32> = samples.iter().step_by(channels).copied().collect();
        mono.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn upsample_44100_to_48000_preserves_tone() {
        let input = sine(44100, 440.0, 44100.0, 2);
        let mut rs = CubicResampler::new(2, 44100, 48000);
        let out = rs.process(&input);
        rs.flush();
        let mut tail = vec![0.0; 4096];
        let n = rs.pull(&mut tail);
        let mut full = out;
        full.extend_from_slice(&tail[..n]);

        // Frame count within one frame of the ideal ratio.
        let frames = full.len() / 2;
        assert!((frames as i64 - 48000).abs() <= 1, "frames = {frames}");
        // Same pitch and duration: zero-crossings count cycles per
        // second of audio, which resampling must preserve.
        let zc_in = zero_crossings(&input, 2);
        let zc_out = zero_crossings(&full, 2);
        let ratio = zc_out as f64 / zc_in as f64;
        assert!((ratio - 1.0).abs() < 0.02, "ratio = {ratio}");
        // No clipping introduced.
        assert!(full.iter().all(|s| s.abs() <= 1.0), "no clipping");
    }

    #[test]
    fn identity_rate_is_transparent() {
        let input = sine(8000, 1000.0, 44100.0, 1);
        let mut rs = CubicResampler::new(1, 44100, 44100);
        let mut out = rs.process(&input);
        rs.flush();
        let mut tail = vec![0.0; 1024];
        let n = rs.pull(&mut tail);
        out.extend_from_slice(&tail[..n]);
        assert_eq!(out.len(), input.len());
        for (a, b) in out.iter().zip(input.iter()) {
            assert!((a - b).abs() < 1e-4, "transparent at 1:1");
        }
    }

    #[test]
    fn downsample_48000_to_44100_frame_count() {
        let input = sine(48000, 440.0, 48000.0, 2);
        let mut rs = CubicResampler::new(2, 48000, 44100);
        let mut out = rs.process(&input);
        rs.flush();
        let mut tail = vec![0.0; 4096];
        loop {
            let n = rs.pull(&mut tail);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&tail[..n]);
        }
        let frames = out.len() / 2;
        assert!((frames as i64 - 44100).abs() <= 1, "frames = {frames}");
    }
}
