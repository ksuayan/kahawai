//! Shared-mode PCM output via cpal.
//!
//! Contract with the engine:
//! - `open()` negotiates the device config: the track's channel count is
//!   used when the device offers it; otherwise (a mono audiobook on a
//!   stereo-only output) the device opens with more channels and `write()`
//!   spreads the track's channels over them (mono goes to both left and
//!   right). The engine never sees the difference. The track's sample
//!   rate is used when the device supports it, otherwise the device rate
//!   is used and the engine resamples (`preferred_sample_rate` reports
//!   the real rate — hence "native rate where supported, resample only
//!   if required").
//! - The audio callback only drains the ring buffer and writes silence
//!   on underrun: no allocation, no decode, no locking beyond the
//!   lock-free ring. `write()` blocks (with a deadline) when the ring is
//!   full, giving the engine natural backpressure.
//! - Underruns are counted, never hidden.
//! - Transitions are click-free: the callback runs a [`Fader`], so pause, stop,
//!   seek and skip fade the already-queued audio out (about 8 ms) instead of
//!   cutting it mid-waveform, resume and an interrupted stream's start fade in,
//!   and an underrun fades what is left instead of dropping to silence. A track
//!   that ends cleanly (the ring drained) hands over to the next untouched.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use kahawai_core::{MusicError, Track};
use kahawai_player_core::{AudioSink, Fader, OutputPath, PcmChunk, SinkState};
use ringbuf::{traits::*, HeapRb};

/// One second of headroom at the highest rate we expect to see, in
/// samples (f32 per channel). Local playback never needs more; the
/// engine pumps far ahead of the callback.
const RING_SECONDS: u32 = 1;
const RING_MAX_RATE: u32 = 192_000;
const RING_MAX_CHANNELS: usize = 8;
/// How far ahead of the speakers `write()` lets the engine run, in ms of
/// audio. The ring is *allocated* for the worst case (192 kHz x 8 ch), which
/// is ~17 s at 44.1 kHz stereo — filling that far ahead made volume changes
/// land many seconds late and put the playhead far ahead of what was heard.
/// Keep only a short cushion queued; `write()` blocks (backpressure) beyond it.
const TARGET_BUFFER_MS: usize = 200;
/// How long `write()` waits for the device to drain before giving up.
const WRITE_DEADLINE: Duration = Duration::from_secs(2);
/// How long a fade takes: long enough to avoid a click, short enough to be
/// instant to the ear.
const FADE_MS: u32 = 8;
/// The most the engine thread waits for a fade-out to finish (a few callbacks).
const FADE_WAIT: Duration = Duration::from_millis(40);

/// Where the output callback gets audio: the ring in production, a `Vec` in tests.
trait Source {
    fn pop(&mut self, out: &mut [f32]) -> usize;
}

impl Source for ringbuf::HeapCons<f32> {
    fn pop(&mut self, out: &mut [f32]) -> usize {
        self.pop_slice(out)
    }
}

/// What the output callback does with one buffer. `audible` is what the engine
/// wants (false while pausing or stopping). Returns true on a real underrun
/// (the engine did not keep up), which is counted and never hidden.
fn render<S: Source>(
    src: &mut S,
    fader: &mut Fader,
    audible: bool,
    data: &mut [f32],
    channels: usize,
) -> bool {
    fader.set_audible(audible);
    if fader.is_silent() {
        // Paused or stopped and faded out: play nothing and leave the queued
        // audio alone, so resuming carries on exactly where it stopped.
        data.fill(0.0);
        return false;
    }
    let n = src.pop(data);
    if n < data.len() {
        fader.underrun(data, n, channels);
        return audible;
    }
    fader.process(data, channels);
    false
}

pub struct CpalSink {
    host: cpal::Host,
    /// Chosen output device by name; `None` = the system default.
    device_name: Option<String>,
    stream: Option<cpal::Stream>,
    producer: Option<ringbuf::HeapProd<f32>>,
    stream_rate: Option<u32>,
    /// Channels the device was opened with.
    channels: u16,
    /// Channels the engine writes (the track's), when fewer than `channels`.
    src_channels: u16,
    /// Reused buffer for upmixed audio.
    upmix: Vec<f32>,
    underruns: Arc<AtomicU64>,
    stream_error: Arc<AtomicBool>,
    /// What the engine wants the callback to do: false fades it out.
    audible: Arc<AtomicBool>,
    /// Set by the callback once it has faded out completely.
    silent: Arc<AtomicBool>,
    /// The last stream drained to its end (a natural track change), so the next
    /// one starts untouched and nothing needs fading out.
    clean_end: bool,
    state: SinkState,
}

impl CpalSink {
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
            device_name: None,
            stream: None,
            producer: None,
            stream_rate: None,
            channels: 0,
            src_channels: 0,
            upmix: Vec::new(),
            underruns: Arc::new(AtomicU64::new(0)),
            stream_error: Arc::new(AtomicBool::new(false)),
            audible: Arc::new(AtomicBool::new(true)),
            silent: Arc::new(AtomicBool::new(false)),
            clean_end: false,
            state: SinkState::Stopped,
        }
    }

    /// The chosen device, or the system default when none is chosen or the
    /// chosen one is not connected any more (unplugged DAC, sleeping
    /// Bluetooth speaker): playback keeps working rather than failing.
    fn pick_device(&self) -> Result<cpal::Device, MusicError> {
        if let Some(want) = &self.device_name {
            let found =
                self.host.output_devices().ok().and_then(|mut it| {
                    it.find(|d| d.name().ok().as_deref() == Some(want.as_str()))
                });
            match found {
                Some(d) => return Ok(d),
                None => {
                    tracing::warn!(device = %want, "output device not found; using system default")
                }
            }
        }
        self.host
            .default_output_device()
            .ok_or_else(|| MusicError::Audio("no default output device".into()))
    }

    /// Fade the queued audio out and wait (briefly) for the callback to finish,
    /// so what comes next never cuts it off mid-waveform. Nothing to do when
    /// the device is not running or the stream just drained to its end.
    fn fade_out(&self) {
        if self.stream.is_none() || self.state != SinkState::Playing || self.clean_end {
            return;
        }
        self.audible.store(false, Ordering::SeqCst);
        let deadline = Instant::now() + FADE_WAIT;
        while !self.silent.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn close_stream(&mut self) {
        self.fade_out();
        // Dropping the Stream stops the device callback synchronously.
        self.stream = None;
        self.producer = None;
        self.stream_rate = None;
        self.channels = 0;
        self.src_channels = 0;
        self.stream_error.store(false, Ordering::SeqCst);
    }
}

// `cpal::Stream` is `!Send` on macOS (it holds a boxed property-listener
// closure), but the sink is only ever moved to the playback thread once and
// used there; the stream is never shared. Same contract as CoreAudioDopSink.
unsafe impl Send for CpalSink {}

impl Default for CpalSink {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSink for CpalSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        let clean = self.clean_end;
        self.close_stream();
        self.clean_end = false;
        let device = self.pick_device()?;

        let channels = track.channels.unwrap_or(2).clamp(1, 8) as u16;
        let want_rate = track.sample_rate.unwrap_or(44_100);

        let mut ranges: Vec<_> = device
            .supported_output_configs()
            .map_err(|e| MusicError::Audio(format!("output configs: {e}")))?
            .collect();
        // Prefer f32; the callback works in f32 natively.
        ranges.retain(|r| r.sample_format() == cpal::SampleFormat::F32);
        let out_channels = output_channels(channels, ranges.iter().map(|r| r.channels()))
            .ok_or_else(|| MusicError::Audio(format!("no {channels}-channel f32 output config")))?;
        ranges.retain(|r| r.channels() == out_channels);
        let range = ranges
            .iter()
            .find(|r| r.min_sample_rate().0 <= want_rate && want_rate <= r.max_sample_rate().0)
            .or_else(|| ranges.first())
            .ok_or_else(|| MusicError::Audio(format!("no {channels}-channel f32 output config")))?;
        if out_channels != channels {
            tracing::info!(
                track_channels = channels,
                device_channels = out_channels,
                "device has no matching channel layout; upmixing"
            );
        }
        let rate = want_rate.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
        let config = range.with_sample_rate(cpal::SampleRate(rate)).config();

        let cap = (RING_SECONDS * RING_MAX_RATE) as usize * RING_MAX_CHANNELS;
        let (prod, mut cons) = HeapRb::<f32>::new(cap).split();
        let underruns = self.underruns.clone();
        let stream_error = self.stream_error.clone();
        // A stream that follows an interrupted one (seek, skip, a new pick)
        // fades in; one that follows a clean end starts untouched.
        self.audible.store(true, Ordering::SeqCst);
        self.silent.store(false, Ordering::SeqCst);
        let audible = self.audible.clone();
        let silent = self.silent.clone();
        let ch = config.channels as usize;
        let mut fader = Fader::new(
            (config.sample_rate.0 * FADE_MS / 1000).max(32),
            if clean { 1.0 } else { 0.0 },
        );
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // Drain only: no allocation, no decode, no blocking.
                    // A shortfall means the engine didn't keep up: count it,
                    // fade what there is and emit silence rather than repeat
                    // audio or cut it off.
                    if render(
                        &mut cons,
                        &mut fader,
                        audible.load(Ordering::Relaxed),
                        data,
                        ch,
                    ) {
                        underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    silent.store(fader.is_silent(), Ordering::Relaxed);
                },
                move |err| {
                    tracing::warn!("cpal output stream error: {err}");
                    stream_error.store(true, Ordering::SeqCst);
                },
                None,
            )
            .map_err(|e| MusicError::Audio(format!("cpal stream: {e}")))?;

        self.producer = Some(prod);
        self.stream_rate = Some(config.sample_rate.0);
        self.channels = config.channels;
        self.src_channels = channels;
        self.stream = Some(stream);
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        if self.stream_error.load(Ordering::SeqCst) {
            return Err(MusicError::Audio("output device error".into()));
        }
        let frames: &[f32] = if chunk.channels as u16 == self.channels {
            &chunk.frames
        } else if chunk.channels as u16 == self.src_channels {
            upmix(
                &chunk.frames,
                chunk.channels as usize,
                self.channels as usize,
                &mut self.upmix,
            );
            &self.upmix
        } else {
            return Err(MusicError::Audio(format!(
                "channel mismatch: chunk has {}, device opened {}",
                chunk.channels, self.channels
            )));
        };
        self.clean_end = false; // new audio is queued: the old stream is not what ends
        let prod = self
            .producer
            .as_mut()
            .ok_or_else(|| MusicError::Audio("sink not open".into()))?;
        let mut rest = frames;
        let deadline = Instant::now() + WRITE_DEADLINE;
        // Backpressure: wait until the queued audio is down to the target.
        let target = self.stream_rate.unwrap_or(48_000) as usize
            * self.channels.max(1) as usize
            * TARGET_BUFFER_MS
            / 1000;
        while prod.occupied_len() > target {
            if Instant::now() >= deadline {
                return Err(MusicError::Audio(
                    "output ring full: device not draining".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        while !rest.is_empty() {
            let n = prod.push_slice(rest);
            rest = &rest[n..];
            if !rest.is_empty() {
                if Instant::now() >= deadline {
                    return Err(MusicError::Audio(
                        "output ring full: device not draining".into(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(())
    }

    fn play(&mut self) -> Result<(), MusicError> {
        let s = self
            .stream
            .as_ref()
            .ok_or_else(|| MusicError::Audio("sink not open".into()))?;
        // Ask for audio before the device starts, so the first callback fades in.
        self.audible.store(true, Ordering::SeqCst);
        s.play()
            .map_err(|e| MusicError::Audio(format!("cpal play: {e}")))?;
        self.state = SinkState::Playing;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), MusicError> {
        let s = self
            .stream
            .as_ref()
            .ok_or_else(|| MusicError::Audio("sink not open".into()))?;
        // Fade out first: halting the device mid-waveform is a click.
        self.fade_out();
        s.pause()
            .map_err(|e| MusicError::Audio(format!("cpal pause: {e}")))?;
        self.state = SinkState::Paused;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MusicError> {
        // Drop the stream: the next open() rebuilds it. Pause keeps the
        // stream (resume continues); stop fully releases the device.
        self.close_stream();
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn state(&self) -> SinkState {
        self.state
    }

    fn preferred_sample_rate(&self) -> Option<u32> {
        self.stream_rate
    }

    fn select_output_path(&mut self, _path: OutputPath) {
        // This sink *is* the PCM path; the router only sends PCM here.
    }

    fn set_output_device(&mut self, name: Option<&str>) {
        let name = name.map(str::to_owned);
        if name != self.device_name {
            self.device_name = name;
            // Release the old device now; the next open() builds the
            // stream on the new one.
            self.close_stream();
            self.state = SinkState::Stopped;
        }
    }

    fn buffered_frames(&self) -> u64 {
        match (&self.producer, self.channels) {
            (Some(p), ch) if ch > 0 => (p.occupied_len() / ch as usize) as u64,
            _ => 0,
        }
    }

    fn drain(&mut self) {
        // Only a running stream empties its ring; a paused/stopped one
        // would never drain, so don't wait on it.
        if self.state != SinkState::Playing {
            return;
        }
        let rate = self.stream_rate.unwrap_or(48_000).max(1) as u64;
        let ch = self.channels.max(1) as usize;
        let Some(p) = self.producer.as_ref() else {
            return;
        };
        // Bounded by the audio that is actually queued, plus slack.
        let queued_ms = (p.occupied_len() / ch) as u64 * 1000 / rate;
        let deadline = Instant::now() + Duration::from_millis(queued_ms + 500);
        while p.occupied_len() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Played out to the end: whatever follows starts untouched.
        self.clean_end = p.occupied_len() == 0;
    }

    fn underrun_count(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

/// The channel count to open the device with for a track of `want`
/// channels: `want` itself when offered, else stereo for a mono track, else
/// the smallest layout with more channels. `None` when every layout has
/// fewer channels (the engine does not downmix).
fn output_channels(want: u16, offered: impl Iterator<Item = u16>) -> Option<u16> {
    let offered: Vec<u16> = offered.collect();
    if offered.contains(&want) {
        return Some(want);
    }
    if want == 1 && offered.contains(&2) {
        return Some(2);
    }
    offered.into_iter().filter(|&c| c > want).min()
}

/// Spread interleaved `src` audio of `from` channels over `to` channels
/// into `out`: channel `c` goes to output `c`, the rest are silent, except
/// that mono is copied to the first two outputs (left and right).
fn upmix(src: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    out.clear();
    out.resize(src.len() / from * to, 0.0);
    for (i, frame) in src.chunks_exact(from).enumerate() {
        let dst = &mut out[i * to..(i + 1) * to];
        dst[..from].copy_from_slice(frame);
        if from == 1 && to >= 2 {
            dst[1] = frame[0];
        }
    }
}

/// One cpal output device, for the shell's device list.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// v1 identifier: the cpal device name (no stable ids in cpal).
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Enumerate output devices. Never fails hard: a device whose name can't
/// be read shows up as "(unnamed)".
pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            let name = device.name().unwrap_or_else(|_| "(unnamed)".into());
            out.push(DeviceInfo {
                id: name.clone(),
                name: name.clone(),
                is_default: default_name.as_deref() == Some(name.as_str()),
            });
        }
    }
    out
}

#[cfg(test)]
mod upmix_tests {
    use super::*;

    #[test]
    fn a_layout_the_device_offers_is_used_as_is() {
        assert_eq!(output_channels(2, [2, 8].into_iter()), Some(2));
        assert_eq!(output_channels(1, [1, 2].into_iter()), Some(1));
    }

    #[test]
    fn mono_opens_stereo_and_wider_falls_back_to_the_next_size_up() {
        assert_eq!(output_channels(1, [8, 2, 4].into_iter()), Some(2));
        assert_eq!(output_channels(1, [8, 4].into_iter()), Some(4));
        assert_eq!(output_channels(3, [2, 8, 6].into_iter()), Some(6));
    }

    #[test]
    fn a_device_with_fewer_channels_is_refused() {
        assert_eq!(output_channels(6, [2].into_iter()), None);
        assert_eq!(output_channels(1, std::iter::empty()), None);
    }

    #[test]
    fn mono_is_heard_on_left_and_right() {
        let mut out = Vec::new();
        upmix(&[0.1, -0.2, 0.3], 1, 2, &mut out);
        assert_eq!(out, [0.1, 0.1, -0.2, -0.2, 0.3, 0.3]);
        upmix(&[0.5], 1, 4, &mut out);
        assert_eq!(out, [0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn extra_channels_are_silent() {
        let mut out = vec![9.0; 3];
        upmix(&[0.1, 0.2, 0.3, 0.4], 2, 4, &mut out);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.0, 0.3, 0.4, 0.0, 0.0]);
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;

    fn name_of(d: &cpal::Device) -> Option<String> {
        d.name().ok()
    }

    /// Picking by name must land on exactly that device (names may carry
    /// trailing spaces / non-ASCII), and an unknown or unplugged name must
    /// fall back to the system default instead of failing playback.
    #[test]
    fn pick_device_honours_the_name_and_falls_back() {
        let mut sink = CpalSink::new();
        for d in list_output_devices() {
            sink.set_output_device(Some(&d.name));
            let got = sink.pick_device().expect("named device is pickable");
            assert_eq!(name_of(&got).as_deref(), Some(d.name.as_str()));
        }

        sink.set_output_device(Some("definitely not a connected device"));
        let fallback = sink.pick_device();
        let default = cpal::default_host().default_output_device();
        assert_eq!(
            fallback.ok().and_then(|d| name_of(&d)),
            default.and_then(|d| name_of(&d))
        );

        sink.set_output_device(None);
        assert!(sink.device_name.is_none());
    }

    #[test]
    fn changing_device_releases_the_open_stream() {
        let mut sink = CpalSink::new();
        sink.set_output_device(Some("A"));
        assert_eq!(sink.device_name.as_deref(), Some("A"));
        assert!(sink.stream.is_none() && sink.producer.is_none());
    }
}

#[cfg(test)]
mod fade_tests {
    use super::*;

    const RATE: u32 = 48_000;
    const CH: usize = 2;
    const BUF: usize = 480; // a 10 ms device buffer

    /// A queue of audio the "device" drains, remembering how much it gave out.
    struct Queue {
        data: Vec<f32>,
        at: usize,
    }

    impl Source for Queue {
        fn pop(&mut self, out: &mut [f32]) -> usize {
            let n = out.len().min(self.data.len() - self.at);
            out[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            n
        }
    }

    fn sine(freq: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin();
                [v, v]
            })
            .collect()
    }

    /// Largest jump between consecutive frames (left channel).
    fn max_step(x: &[f32]) -> f32 {
        let m: Vec<f32> = x.iter().step_by(CH).copied().collect();
        m.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    fn natural(freq: f32, amp: f32) -> f32 {
        2.0 * std::f32::consts::PI * freq / RATE as f32 * amp
    }

    /// Run the callback for `buffers` device buffers.
    fn run(q: &mut Queue, f: &mut Fader, audible: bool, buffers: usize) -> (Vec<f32>, usize) {
        let (mut out, mut underruns) = (Vec::new(), 0);
        for _ in 0..buffers {
            let mut buf = vec![7.0f32; BUF * CH];
            if render(q, f, audible, &mut buf, CH) {
                underruns += 1;
            }
            out.extend(buf);
        }
        (out, underruns)
    }

    fn fader(initial: f32) -> Fader {
        Fader::new(RATE * FADE_MS / 1000, initial)
    }

    #[test]
    fn steady_playback_is_untouched() {
        let src = sine(220.0, 0.7, BUF * 10);
        let mut q = Queue {
            data: src.clone(),
            at: 0,
        };
        let (out, underruns) = run(&mut q, &mut fader(1.0), true, 10);
        assert_eq!(out, src, "bit-exact when nothing is fading");
        assert_eq!(underruns, 0);
    }

    #[test]
    fn pausing_fades_out_without_a_click_and_stops_consuming_the_queue() {
        let (freq, amp) = (60.0, 0.8);
        let mut q = Queue {
            data: sine(freq, amp, BUF * 60),
            at: 0,
        };
        let mut f = fader(1.0);
        let (mut heard, _) = run(&mut q, &mut f, true, 3);
        let (faded, _) = run(&mut q, &mut f, false, 3); // pause requested
        heard.extend(faded);
        assert!(
            max_step(&heard) <= natural(freq, amp) * 1.05 + amp / (RATE * FADE_MS / 1000) as f32,
            "pause stepped the signal by {}",
            max_step(&heard)
        );
        assert!(f.is_silent());
        let consumed = q.at;
        let (silence, underruns) = run(&mut q, &mut f, false, 5);
        assert!(silence.iter().all(|&s| s == 0.0));
        assert_eq!(q.at, consumed, "paused: the queued audio is left alone");
        assert_eq!(underruns, 0, "a pause is not an underrun");
    }

    #[test]
    fn resuming_carries_on_where_it_stopped_and_fades_in() {
        let (freq, amp) = (60.0, 0.8);
        let all = sine(freq, amp, BUF * 60);
        let mut q = Queue {
            data: all.clone(),
            at: 0,
        };
        let mut f = fader(1.0);
        run(&mut q, &mut f, true, 3);
        run(&mut q, &mut f, false, 3);
        let resume_from = q.at;
        let (out, _) = run(&mut q, &mut f, true, 4);
        assert!(
            out[0].abs() < 0.8 * 0.01,
            "fades in from silence: {}",
            out[0]
        );
        assert!(max_step(&out) <= natural(freq, amp) * 1.05 + amp / (RATE * FADE_MS / 1000) as f32);
        // once the fade is done the audio is exactly the queue's next samples
        let done = (RATE * FADE_MS / 1000) as usize * CH;
        assert_eq!(
            &out[done..],
            &all[resume_from + done..resume_from + out.len()]
        );
    }

    #[test]
    fn a_stream_after_an_interruption_fades_in_and_after_a_clean_end_does_not() {
        let src = sine(100.0, 0.6, BUF * 4);
        let mut q = Queue {
            data: src.clone(),
            at: 0,
        };
        let (fresh, _) = run(&mut q, &mut fader(0.0), true, 2);
        assert!(
            fresh[0].abs() < 0.01
                && max_step(&fresh)
                    <= natural(100.0, 0.6) * 1.05 + 0.6 / (RATE * FADE_MS / 1000) as f32
        );
        let mut q2 = Queue {
            data: src.clone(),
            at: 0,
        };
        let (gapless, _) = run(&mut q2, &mut fader(1.0), true, 2);
        assert_eq!(
            gapless,
            src[..BUF * CH * 2],
            "a clean track change starts untouched"
        );
    }

    #[test]
    fn an_underrun_is_counted_and_faded_not_cut() {
        let (freq, amp) = (60.0, 0.8);
        // 1.4 buffers of audio: the second buffer runs dry part-way.
        let mut q = Queue {
            data: sine(freq, amp, BUF + BUF * 2 / 5),
            at: 0,
        };
        let mut f = fader(1.0);
        let (out, underruns) = run(&mut q, &mut f, true, 2);
        assert_eq!(underruns, 1, "counted, never hidden");
        assert!(max_step(&out) <= natural(freq, amp) * 1.05 + amp / (RATE * FADE_MS / 1000) as f32);
        assert!(out[out.len() - 1].abs() < 1e-6, "ends in silence");
        // audio returns: it fades back in rather than starting at full level
        let mut q2 = Queue {
            data: sine(freq, amp, BUF * 4),
            at: 0,
        };
        let (back, _) = run(&mut q2, &mut f, true, 3);
        assert!(back[0].abs() < 0.01);
        assert!(
            max_step(&back) <= natural(freq, amp) * 1.05 + amp / (RATE * FADE_MS / 1000) as f32
        );
    }

    #[test]
    fn an_idle_source_while_paused_is_not_an_underrun() {
        let mut q = Queue {
            data: Vec::new(),
            at: 0,
        };
        let mut f = fader(1.0);
        let (_, underruns) = run(&mut q, &mut f, false, 20);
        assert_eq!(underruns, 0);
    }
}
