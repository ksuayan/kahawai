//! Click-free gain changes on track boundaries.

/// Linear ramp between gains, applied per sample. The engine retargets it
/// on every track open; consecutive tracks with different loudness gains
/// crossfade gains (not audio) over ~50 ms — no clicks, no pumping.
pub struct GainRamp {
    current: f32,
    target: f32,
    step: f32,
    ramp_frames: usize,
}

impl GainRamp {
    pub fn new(ramp_frames: usize) -> Self {
        Self {
            current: 1.0,
            target: 1.0,
            step: 0.0,
            ramp_frames: ramp_frames.max(1),
        }
    }

    /// Jump to `db` at once (the start of a stream: nothing to smooth).
    pub fn snap(&mut self, db: f32) {
        let g = 10f32.powf(db / 20.0);
        self.current = g;
        self.target = g;
        self.step = 0.0;
    }

    pub fn retarget(&mut self, target_db: f32) {
        let target = 10f32.powf(target_db / 20.0);
        self.step = (target - self.current) / self.ramp_frames as f32;
        self.target = target;
    }

    /// Target gain in dB (for tests).
    #[cfg(test)]
    fn target_db(&self) -> f32 {
        20.0 * self.target.log10()
    }

    pub fn apply(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            if (self.current - self.target).abs() > f32::EPSILON {
                self.current += self.step;
                // Clamp overshoot from float accumulation.
                if (self.step > 0.0 && self.current > self.target)
                    || (self.step < 0.0 && self.current < self.target)
                {
                    self.current = self.target;
                }
            }
            *s *= self.current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_ramp_reaches_target_without_click() {
        let mut ramp = GainRamp::new(100);
        ramp.retarget(6.0); // +6 dB ≈ ×1.995
        assert!((ramp.target_db() - 6.0).abs() < 1e-4);
        let mut buf = vec![1.0f32; 200];
        ramp.apply(&mut buf);
        // First sample ramps from 1.0, last samples sit at the target.
        assert!(buf[0] > 1.0 && buf[0] < 1.995);
        assert!((buf[199] - 1.995).abs() < 0.01);
        // Monotonic: no discontinuities.
        for w in buf.windows(2) {
            assert!(w[1] >= w[0] - 1e-6);
        }
    }
}
