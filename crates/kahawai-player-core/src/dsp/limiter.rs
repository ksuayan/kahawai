//! The last stages of the shared PCM path: the look-ahead limiter, and the
//! headroom guard behind it as a defense in depth.

use std::collections::VecDeque;

/// Where the headroom guard starts to bend the signal (about -0.9 dBFS).
pub const GUARD_THRESHOLD: f32 = 0.9;

/// Headroom guard for the shared (DSP) path. EQ boosts and loudness gain
/// (up to +12 dB) can push peaks past full scale, and the output device
/// hard-clips anything over 1.0, which is harsh on loud transients. This
/// bends only what rises above [`GUARD_THRESHOLD`] along a smooth curve
/// that approaches, and never exceeds, 1.0. Everything below the threshold
/// is untouched, so ordinary material passes bit-for-bit.
pub fn headroom_guard(samples: &mut [f32]) {
    const T: f32 = GUARD_THRESHOLD;
    for s in samples.iter_mut() {
        let a = s.abs();
        if a > T {
            let over = (a - T) / (1.0 - T);
            *s = s.signum() * (T + (1.0 - T) * over.tanh());
        }
    }
}

/// Ceiling the limiter guarantees the signal never exceeds. Matches [`GUARD_THRESHOLD`], so
/// [`headroom_guard`] (kept as a defense-in-depth no-op for correctly limited audio) never has
/// to bend anything.
pub const LIMITER_CEILING: f32 = GUARD_THRESHOLD;
/// How far ahead the limiter sees a peak coming. Long enough to duck ahead of a fast transient,
/// short enough to be an imperceptible constant delay.
pub const LIMITER_LOOKAHEAD_MS: f32 = 5.0;
/// Gain-recovery time constant after a peak passes. Slow enough that recovery is not audible as
/// pumping; fast enough not to duck a whole quiet passage after one loud moment.
pub const LIMITER_RELEASE_MS: f32 = 60.0;

/// True look-ahead limiter for the shared PCM path (the final stage, after EQ, analog character,
/// loudness gain and volume). Unlike [`headroom_guard`], which can only react to a peak already
/// at the output — a soft clip for a large overshoot — this delays the signal by a fixed
/// [`LookaheadLimiter::latency_frames`] and starts reducing gain *before* the peak arrives, so
/// [`LIMITER_CEILING`] is reached exactly rather than bent into. This matters most with loudness
/// normalization off, where nothing else plans for an EQ boost's worst case.
///
/// Stereo/multi-channel linked: one gain curve applies to every channel (the peak across
/// channels drives it), so the image is preserved.
///
/// Unlike [`DspStage`]'s other implementors, this does *not* process same-length blocks in
/// place: doing that honestly (rather than papering over it with a leading silence pad and a
/// dropped tail) needs the output to grow or shrink. `process` therefore takes `&mut Vec<f32>`.
/// The very first block after a [`reset`](Self::reset) comes back exactly
/// [`Self::latency_frames`] frames *shorter* than it went in — nothing is invented — and
/// [`Self::flush`] hands back the frames still held once a track's decoder is genuinely
/// exhausted, so no real audio is dropped and no silence is added: total frame count in equals
/// total frame count out (regular blocks plus one flush), keeping gapless boundaries sample-exact.
///
/// Algorithm: a monotonic deque gives the sliding-window minimum of the per-frame "gain needed
/// to keep this frame under the ceiling" over the look-ahead window in O(1) amortized time. That
/// minimum is always a valid upper bound for the current output frame's gain (it is a min over a
/// window that includes the output frame's own requirement), so gain drops to it immediately —
/// safe, and inaudible because it happens *before* the loud material that justifies it
/// (pre-masking). Recovery afterward is a one-pole ramp toward the target
/// ([`LIMITER_RELEASE_MS`]), so it never snaps back up right after a transient (no pumping), and
/// — since the ramp only rises toward a target it never overtakes — the ceiling guarantee holds
/// throughout.
pub struct LookaheadLimiter {
    channels: usize,
    lookahead_frames: usize,
    release_coef: f32,
    /// Delayed dry signal, interleaved. Between calls, holds exactly
    /// `min(lookahead_frames, next_in - next_out)` complete frames — full once warmed up, fewer
    /// only while filling right after a reset.
    delay: VecDeque<f32>,
    /// Monotonic deque of `(frame index, required gain)`: increasing index, non-decreasing gain
    /// front-to-back, so the front is always the window's minimum.
    window: VecDeque<(u64, f32)>,
    next_in: u64,
    next_out: u64,
    current_gain: f32,
    /// Deepest gain applied since the last [`LookaheadLimiter::take_reduction_db`]
    /// — peak-hold, so a meter reading at any rate still sees short reductions
    /// it would otherwise sample straight past.
    min_gain_since_read: f32,
}

fn release_coefficient(sample_rate: u32, release_ms: f32) -> f32 {
    let dt = 1.0 / sample_rate.max(1) as f32;
    1.0 - (-dt / (release_ms / 1000.0)).exp()
}

impl LookaheadLimiter {
    pub fn new(sample_rate: u32) -> Self {
        let mut s = Self {
            channels: 0,
            lookahead_frames: 1,
            release_coef: 1.0,
            delay: VecDeque::new(),
            window: VecDeque::new(),
            next_in: 0,
            next_out: 0,
            current_gain: 1.0,
            min_gain_since_read: 1.0,
        };
        s.prepare(sample_rate);
        s
    }

    /// Re-tune for a new sample rate and forget all history (a genuine discontinuity — a new
    /// track or a seek — never mid-track).
    pub fn prepare(&mut self, sample_rate: u32) {
        self.lookahead_frames =
            ((sample_rate.max(1) as f32 * LIMITER_LOOKAHEAD_MS / 1000.0).round() as usize).max(1);
        self.release_coef = release_coefficient(sample_rate, LIMITER_RELEASE_MS);
        self.reset();
    }

    /// Forget history: drops any buffered audio and resets gain to unity. Frames still held (not
    /// yet flushed) are gone — call [`Self::flush`] first if they matter (a genuine end of
    /// stream); call this alone for an abandoned stream (skip, seek), where discarding them is
    /// correct.
    pub fn reset(&mut self) {
        self.delay.clear();
        self.window.clear();
        self.next_in = 0;
        self.next_out = 0;
        self.current_gain = 1.0;
        self.min_gain_since_read = 1.0;
    }

    /// Deepest gain reduction applied since the last call, in dB (0.0 = none,
    /// larger = more reduction), then starts a fresh interval from wherever
    /// the envelope currently sits. Peak-hold: a caller reading at any rate
    /// still sees a brief duck it would otherwise sample past, which is what
    /// makes a gain-reduction meter honest without a high-rate metering
    /// channel. Cheap by construction — the minimum is tracked in the loop
    /// that was already applying the gain.
    pub fn take_reduction_db(&mut self) -> f32 {
        let min = self.min_gain_since_read.clamp(1e-6, 1.0);
        self.min_gain_since_read = self.current_gain;
        -20.0 * min.log10()
    }

    /// Frames of latency this stage adds to the shared PCM output.
    pub fn latency_frames(&self) -> u32 {
        self.lookahead_frames as u32
    }

    fn ensure_channels(&mut self, channels: usize) {
        if self.channels != channels {
            self.channels = channels;
            self.reset();
        }
    }

    /// Required gain for one frame: 1.0 if its peak is already under the ceiling, else exactly
    /// enough to bring it to the ceiling.
    fn required_gain(frame: &[f32]) -> f32 {
        let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        if peak > LIMITER_CEILING {
            LIMITER_CEILING / peak
        } else {
            1.0
        }
    }

    fn push_frame(&mut self, frame: &[f32]) {
        let required = Self::required_gain(frame);
        self.delay.extend(frame.iter().copied());
        while matches!(self.window.back(), Some(&(_, g)) if g >= required) {
            self.window.pop_back();
        }
        self.window.push_back((self.next_in, required));
        self.next_in += 1;
    }

    /// Pops one frame's worth of delayed samples, advances the gain envelope toward the window's
    /// current minimum, and writes the result (scaled) into `out`. Only valid while at least one
    /// frame is buffered.
    fn emit_frame(&mut self, out: &mut Vec<f32>) {
        while matches!(self.window.front(), Some(&(idx, _)) if idx < self.next_out) {
            self.window.pop_front();
        }
        let target = self.window.front().map(|&(_, g)| g).unwrap_or(1.0);
        if target < self.current_gain {
            self.current_gain = target;
        } else {
            self.current_gain += (target - self.current_gain) * self.release_coef;
        }
        // Belt-and-suspenders: the math above never overtakes `target`, but float error should
        // never be allowed to push a frame over ceiling.
        self.current_gain = self.current_gain.min(target);
        self.min_gain_since_read = self.min_gain_since_read.min(self.current_gain);
        for _ in 0..self.channels {
            out.push(self.delay.pop_front().unwrap_or(0.0) * self.current_gain);
        }
        self.next_out += 1;
    }

    /// Processes one block, replacing it in place: pushes every incoming frame, then emits as
    /// many as keeps exactly `latency_frames()` buffered afterward (see the struct docs for why
    /// the length can differ from the input, and why that is not lost or invented audio).
    pub fn process(&mut self, samples: &mut Vec<f32>, channels: usize) {
        self.ensure_channels(channels.max(1));
        if channels == 0 {
            samples.clear();
            return;
        }
        let frames_in = samples.len() / channels;
        for f in 0..frames_in {
            self.push_frame(&samples[f * channels..(f + 1) * channels]);
        }
        let queued = (self.next_in - self.next_out) as usize;
        let emit = queued.saturating_sub(self.lookahead_frames);
        let mut out = Vec::with_capacity(emit * channels);
        for _ in 0..emit {
            self.emit_frame(&mut out);
        }
        *samples = out;
    }

    /// Drains every frame still held in the look-ahead delay (no more input is coming — a
    /// track's decoder is genuinely exhausted, not merely a gapless segment boundary within one
    /// continuous chained stream) and resets. Call this once, before the stream is considered
    /// finished, so the trailing [`Self::latency_frames`] of real audio are not lost.
    pub fn flush(&mut self) -> Vec<f32> {
        let remaining = (self.next_in - self.next_out) as usize;
        let mut out = Vec::with_capacity(remaining * self.channels.max(1));
        for _ in 0..remaining {
            self.emit_frame(&mut out);
        }
        self.reset();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::test_util::sine;

    #[test]
    fn headroom_guard_leaves_normal_material_bit_exact() {
        let mut v: Vec<f32> = (0..1000)
            .map(|i| ((i as f32) * 0.37).sin() * GUARD_THRESHOLD)
            .collect();
        let before = v.clone();
        headroom_guard(&mut v);
        assert_eq!(v, before, "nothing at or below the threshold changes");
    }

    #[test]
    fn headroom_guard_never_exceeds_full_scale_and_keeps_order() {
        let mut prev = 0.0f32;
        for i in 0..=4000 {
            let x = i as f32 * 0.001; // 0.0 ..= 4.0
            let mut a = [x, -x];
            headroom_guard(&mut a);
            assert!(a[0] <= 1.0 && a[1] >= -1.0, "{x} -> {}", a[0]);
            assert_eq!(a[0], -a[1], "symmetric");
            assert!(a[0] >= prev, "monotonic at {x}");
            prev = a[0];
        }
    }

    #[test]
    fn headroom_guard_is_continuous_at_the_threshold() {
        let mut a = [GUARD_THRESHOLD + 1e-4];
        headroom_guard(&mut a);
        assert!(
            (a[0] - (GUARD_THRESHOLD + 1e-4)).abs() < 2e-4,
            "no step where it engages"
        );
    }

    #[test]
    fn headroom_guard_tames_the_measured_overs() {
        // A 1.37 FS peak (track 2 of the reported album) lands just under 1.0.
        let mut a = [1.37f32, -1.06];
        headroom_guard(&mut a);
        assert!(a[0] > 0.99 && a[0] <= 1.0, "{}", a[0]);
        assert!(a[1] < -0.95 && a[1] >= -1.0, "{}", a[1]);
    }

    /// Runs the whole input through in one block, then flushes, so the result covers every real
    /// input frame exactly once, index-aligned (`out[i]` is the processed `input[i]`) with no
    /// invented silence and no dropped tail — see the struct docs on why `process` alone isn't
    /// the whole story.
    fn limiter_process_all(lim: &mut LookaheadLimiter, input: &[f32], channels: usize) -> Vec<f32> {
        let mut chunk = input.to_vec();
        lim.process(&mut chunk, channels);
        chunk.extend(lim.flush());
        chunk
    }

    #[test]
    fn limiter_process_then_flush_accounts_for_every_frame() {
        let mut lim = LookaheadLimiter::new(44_100);
        let input = vec![0.5f32; 2000]; // mono, well under the ceiling
        let out = limiter_process_all(&mut lim, &input, 1);
        assert_eq!(out.len(), input.len(), "no frame invented or dropped");
        assert!(
            out.iter().all(|&s| (s - 0.5).abs() < 1e-6),
            "unchanged, and not delayed in the output"
        );
    }

    #[test]
    fn limiter_process_alone_is_shorter_right_after_a_reset() {
        // Nothing can be emitted before the look-ahead window has filled; `process` alone comes
        // back short rather than padding with silence (`flush` is what closes the gap, once
        // there is truly no more input).
        let mut lim = LookaheadLimiter::new(44_100);
        let lat = lim.latency_frames() as usize;
        let mut chunk = vec![0.1f32; lat * 3];
        lim.process(&mut chunk, 1);
        assert_eq!(
            chunk.len(),
            lat * 2,
            "exactly `lookahead_frames` short on the first block"
        );
    }

    #[test]
    fn limiter_never_exceeds_the_ceiling() {
        let mut lim = LookaheadLimiter::new(44_100);
        // A loud, sudden mono transient among quiet material.
        let mut input = vec![0.1f32; 4000];
        input[2000] = 3.0; // a hard, single-sample spike well past the ceiling
        let out = limiter_process_all(&mut lim, &input, 1);
        let peak = out.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(
            peak <= LIMITER_CEILING + 1e-4,
            "peak {peak} exceeded the ceiling"
        );
    }

    #[test]
    fn limiter_ducks_before_the_peak_arrives_not_after() {
        let mut lim = LookaheadLimiter::new(44_100);
        let mut input = vec![0.1f32; 4000];
        let spike_at = 2000;
        input[spike_at] = 3.0;
        let out = limiter_process_all(&mut lim, &input, 1);
        // Index-aligned output: gain reduction must already be visible in the window *before*
        // the spike's own index, not only at it.
        let before = &out[spike_at - 5..spike_at];
        assert!(
            before.iter().any(|&s| s.abs() < 0.099),
            "no gain reduction visible ahead of the peak: {before:?}"
        );
    }

    #[test]
    fn limiter_is_transparent_below_the_ceiling() {
        let mut lim = LookaheadLimiter::new(44_100);
        let input = sine(1000.0, 4000, 44100, 0.3);
        let out = limiter_process_all(&mut lim, &input, 1);
        for i in 0..input.len() {
            assert!(
                (out[i] - input[i]).abs() < 1e-5,
                "sample {i} changed: {} vs {}",
                out[i],
                input[i]
            );
        }
    }

    #[test]
    fn limiter_stereo_link_applies_the_same_gain_to_both_channels() {
        let mut lim = LookaheadLimiter::new(44_100);
        let frames = 4000;
        let mut input = vec![0.1f32; frames * 2];
        // A spike on the left channel only must still duck the right channel equally, or the
        // stereo image shifts.
        input[2000 * 2] = 3.0;
        let out = limiter_process_all(&mut lim, &input, 2);
        for f in 0..frames {
            let (l, r) = (out[f * 2], out[f * 2 + 1]);
            // Equal input magnitude in but for the spike itself -> equal gain -> equal output, except at the spike frame.
            if f != 2000 {
                assert!(
                    (l - r).abs() < 1e-6,
                    "channels diverged at frame {f}: {l} vs {r}"
                );
            }
        }
    }

    #[test]
    fn limiter_release_is_gradual_not_instant() {
        let mut lim = LookaheadLimiter::new(44_100);
        let mut input = vec![0.1f32; 100_000];
        let spike_at = 2000;
        input[spike_at] = 3.0;
        let out = limiter_process_all(&mut lim, &input, 1);
        // The instant the spike clears the look-ahead window, the target gain jumps back toward
        // 1.0 — but the smoothed release means the *applied* gain only creeps toward it. Sampled
        // at increasing distances past the spike, output should recover monotonically and
        // slowly, never snapping straight back to 0.1.
        let just_after = out[spike_at + 5].abs();
        let mid = out[spike_at + 3_000].abs();
        let much_later = out[spike_at + 90_000].abs();
        assert!(
            just_after < 0.05,
            "should still be near fully ducked right after: {just_after}"
        );
        assert!(
            mid > just_after && mid < 0.099,
            "should be partway recovered, not instant: {mid}"
        );
        assert!(
            (much_later - 0.1).abs() < 1e-3,
            "close to fully recovered after ~20 release time constants: {much_later}"
        );
    }

    #[test]
    fn limiter_reports_peak_held_gain_reduction_then_starts_a_fresh_interval() {
        let mut lim = LookaheadLimiter::new(44_100);
        // Quiet material only: nothing to reduce.
        let mut quiet = vec![0.1f32; 4000];
        lim.process(&mut quiet, 1);
        assert!(
            lim.take_reduction_db() < 1e-6,
            "no reduction on quiet material"
        );

        // A spike 6 dB over the ceiling should report about 6 dB of reduction,
        // even though it only lasts one frame out of thousands — that is the
        // peak-hold behaviour a meter depends on.
        let over = LIMITER_CEILING * 2.0; // +6.02 dB over
        let mut loud = vec![0.1f32; 4000];
        loud[2000] = over;
        lim.process(&mut loud, 1);
        let gr = lim.take_reduction_db();
        assert!(
            (gr - 6.02).abs() < 0.1,
            "expected about 6 dB of reduction, got {gr}"
        );
    }

    #[test]
    fn limiter_flush_returns_exactly_one_lookahead_window_and_resets() {
        let mut lim = LookaheadLimiter::new(44_100);
        let lat = lim.latency_frames() as usize;
        let mut chunk = vec![0.2f32; lat * 3];
        lim.process(&mut chunk, 1);
        let tail = lim.flush();
        assert_eq!(
            tail.len(),
            lat,
            "flush drains exactly the buffered look-ahead window"
        );
        assert!(
            tail.iter().all(|&s| (s - 0.2).abs() < 1e-6),
            "flushed samples are the real trailing audio"
        );
        // After flush, state is fresh: the next block is short by one full look-ahead window
        // again, same as a brand-new limiter.
        let mut next = vec![0.5f32; lat * 2];
        lim.process(&mut next, 1);
        assert_eq!(next.len(), lat, "reset after flush");
    }
}
