//! Signal generators shared by the DSP stages' tests.

pub(super) fn sine(freq: f32, frames: usize, rate: u32, amp: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * amp)
        .collect()
}

pub(super) fn stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&s| [s, s]).collect()
}
