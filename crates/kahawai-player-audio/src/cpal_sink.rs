//! Shared-mode PCM output via cpal.
//!
//! Contract with the engine:
//! - `open()` negotiates the device config: the track's channel count is
//!   mandatory (the engine does not remap channels); the track's sample
//!   rate is used when the device supports it, otherwise the device rate
//!   is used and the engine resamples (`preferred_sample_rate` reports
//!   the real rate — hence "native rate where supported, resample only
//!   if required").
//! - The audio callback only drains the ring buffer and writes silence
//!   on underrun: no allocation, no decode, no locking beyond the
//!   lock-free ring. `write()` blocks (with a deadline) when the ring is
//!   full, giving the engine natural backpressure.
//! - Underruns are counted, never hidden.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use kahawai_core::{MusicError, Track};
use kahawai_player_core::{AudioSink, OutputPath, PcmChunk, SinkState};
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

pub struct CpalSink {
    host: cpal::Host,
    /// Chosen output device by name; `None` = the system default.
    device_name: Option<String>,
    stream: Option<cpal::Stream>,
    producer: Option<ringbuf::HeapProd<f32>>,
    stream_rate: Option<u32>,
    channels: u16,
    underruns: Arc<AtomicU64>,
    stream_error: Arc<AtomicBool>,
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
            underruns: Arc::new(AtomicU64::new(0)),
            stream_error: Arc::new(AtomicBool::new(false)),
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

    fn close_stream(&mut self) {
        // Dropping the Stream stops the device callback synchronously.
        self.stream = None;
        self.producer = None;
        self.stream_rate = None;
        self.channels = 0;
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
        self.close_stream();
        let device = self.pick_device()?;

        let channels = track.channels.unwrap_or(2).clamp(1, 8) as u16;
        let want_rate = track.sample_rate.unwrap_or(44_100);

        let mut ranges: Vec<_> = device
            .supported_output_configs()
            .map_err(|e| MusicError::Audio(format!("output configs: {e}")))?
            .collect();
        // Prefer f32; the callback works in f32 natively.
        ranges.retain(|r| r.channels() == channels && r.sample_format() == cpal::SampleFormat::F32);
        let range = ranges
            .iter()
            .find(|r| r.min_sample_rate().0 <= want_rate && want_rate <= r.max_sample_rate().0)
            .or_else(|| ranges.first())
            .ok_or_else(|| MusicError::Audio(format!("no {channels}-channel f32 output config")))?;
        let rate = want_rate.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
        let config = range.with_sample_rate(cpal::SampleRate(rate)).config();

        let cap = (RING_SECONDS * RING_MAX_RATE) as usize * RING_MAX_CHANNELS;
        let (prod, mut cons) = HeapRb::<f32>::new(cap).split();
        let underruns = self.underruns.clone();
        let stream_error = self.stream_error.clone();
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // Drain only: no allocation, no decode, no blocking.
                    // A shortfall means the engine didn't keep up — count
                    // it and emit silence rather than repeating audio.
                    let n = cons.pop_slice(data);
                    if n < data.len() {
                        underruns.fetch_add(1, Ordering::Relaxed);
                        data[n..].fill(0.0);
                    }
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
        self.stream = Some(stream);
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        if self.stream_error.load(Ordering::SeqCst) {
            return Err(MusicError::Audio("output device error".into()));
        }
        if chunk.channels as u16 != self.channels {
            return Err(MusicError::Audio(format!(
                "channel mismatch: chunk has {}, device opened {}",
                chunk.channels, self.channels
            )));
        }
        let prod = self
            .producer
            .as_mut()
            .ok_or_else(|| MusicError::Audio("sink not open".into()))?;
        let mut rest = &chunk.frames[..];
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
    }

    fn underrun_count(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
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
