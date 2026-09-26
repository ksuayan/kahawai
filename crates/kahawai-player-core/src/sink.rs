//! Audio sink abstraction: every platform shell implements [`AudioSink`]
//! (cpal/rodio on desktop, AudioTrack on Android, AVAudioPlayer on iOS).
//! (Spec: kahawai-player-design.md "Player core & audio engine".)

use kahawai_core::{MusicError, Track};
use serde::{Deserialize, Serialize};

/// Playback state as seen by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SinkState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// Which audio path is active. PCM is the shared-mode device path with the
/// DSP chain (EQ → loudness → volume); DoP is the exclusive hog-mode path
/// that bypasses all PCM DSP, bit-perfect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputPath {
    #[default]
    #[serde(rename = "pcm-shared")]
    Pcm,
    #[serde(rename = "dop-exclusive")]
    Dop,
}

/// Decoded PCM the player hands to the sink. Interleaved f32, one `frames`
/// block per push. Sample rate / channel count are negotiated at `open`.
#[derive(Debug, Clone)]
pub struct PcmChunk {
    pub frames: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u8,
}

/// Platform audio output. Implementations own the device, the decode
/// pipeline, and gapless handoff; kahawai-player-core owns queue + state.
pub trait AudioSink: Send {
    /// Open the device for the given track. Called on every track change.
    fn open(&mut self, track: &Track) -> Result<(), MusicError>;
    /// Push decoded PCM. May block briefly when the device buffer is full.
    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError>;
    fn play(&mut self) -> Result<(), MusicError>;
    fn pause(&mut self) -> Result<(), MusicError>;
    fn stop(&mut self) -> Result<(), MusicError>;
    fn state(&self) -> SinkState;
    /// Device latency in milliseconds, for A/V sync display. May be 0.
    fn latency_ms(&self) -> u64 {
        0
    }
    /// Sample rate the device wants PCM at. `None` (default) = the sink
    /// accepts whatever the stream delivers and the engine must not
    /// resample. C2's cpal sink returns `Some(device_rate)`; the engine
    /// then inserts its cubic resampler only for mismatched streams.
    fn preferred_sample_rate(&self) -> Option<u32> {
        None
    }
    /// Whether the sink can consume a DoP (DSD-over-PCM) byte stream
    /// directly. The engine refuses `?format=dop` when this is false —
    /// DoP must reach a DSD-capable DAC bit-perfect (C2's CoreAudio
    /// hog-mode sink), never a PCM decoder.
    fn supports_dop(&self) -> bool {
        false
    }

    /// PCM rate the sink can output for a DSD source rate, or `None` when
    /// the device cannot do the required DoP rate. The engine falls back
    /// to DSD→PCM/FLAC when this returns `None`.
    fn dop_output_rate(&self, _dsd_rate_hz: u32) -> Option<u32> {
        None
    }

    /// Switch the active output path. Single-path sinks ignore this;
    /// [`SinkRouter`] swaps between its PCM and DoP devices.
    fn select_output_path(&mut self, _path: OutputPath) {}

    /// Push raw DoP payload bytes (24-bit LE frames). Only called when
    /// [`supports_dop`](Self::supports_dop) is true; the bytes must reach
    /// the DAC untouched — no volume, no EQ, no resample, no dither.
    fn write_dop(&mut self, _bytes: &[u8]) -> Result<(), MusicError> {
        Err(MusicError::BadRequest(
            "DoP output not supported by this sink".into(),
        ))
    }

    /// PCM frames (per channel, at the sink's output rate) accepted by
    /// `write` but not yet played. The engine subtracts this from the
    /// frames it has written so the displayed position tracks what is
    /// audible, not what is merely decoded. 0 = unknown/none.
    fn buffered_frames(&self) -> u64 {
        0
    }

    /// Block (bounded) until everything already written has been played.
    /// Called at a natural end of stream, before the next track's `open`
    /// discards the device buffer, so the tail of a track is never cut.
    fn drain(&mut self) {}

    /// Audio-callback underruns since the sink was created (0 for sinks
    /// without a real-time callback). Surfaced for diagnostics.
    fn underrun_count(&self) -> u64 {
        0
    }
}

/// Headless sink: accepts everything, plays nothing. Used by tests and by
/// shells before their real audio backend is wired.
#[derive(Debug, Default)]
pub struct NullSink {
    state: SinkState,
    opened: Vec<i64>,
    pub chunks_written: u64,
    pub frames_written: u64,
}

impl NullSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Track IDs this sink has opened, in order (gapless handoff testing).
    pub fn opened(&self) -> &[i64] {
        &self.opened
    }
}

impl AudioSink for NullSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        self.opened.push(track.id);
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        self.chunks_written += 1;
        self.frames_written += (chunk.frames.len() / chunk.channels.max(1) as usize) as u64;
        Ok(())
    }

    fn play(&mut self) -> Result<(), MusicError> {
        self.state = SinkState::Playing;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), MusicError> {
        self.state = SinkState::Paused;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MusicError> {
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn state(&self) -> SinkState {
        self.state
    }
}

// --- Future audio engine (spec §"Player core & audio engine") ----------------
// TODO(player): real sink backends behind this trait:
//   - desktop: cpal device + symphonia decode pipeline (gapless via
//     pre-decoded ring buffer, S8), libopus adapter for Opus-in-Ogg.
//   - DSD: native DoP passthrough vs DSD→PCM FIR decimation (spec §2 S5a/S5b).
//   - mobile (later): AudioTrack (Android) / AVAudioPlayer (iOS) shells.

/// Test sink: records every sample pushed. The engine's gapless, seek, and
/// resample tests assert against this instead of real audio hardware.
#[derive(Debug, Default)]
pub struct VecSink {
    /// All interleaved f32 samples, in write order.
    pub samples: Vec<f32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    state: SinkState,
    /// For resample-seam tests: pretend the device wants this rate.
    pub demand_rate: Option<u32>,
    /// Test knob: what `buffered_frames()` reports (audio "queued, not yet played").
    pub buffered: u64,
    /// How many times the engine called `drain()` (natural end of stream).
    pub drains: u32,
}

impl VecSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames recorded (samples / channels).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.unwrap_or(1).max(1) as usize
    }
}

impl AudioSink for VecSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        let _ = track;
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        match (self.sample_rate, self.channels) {
            (Some(r), Some(c)) => {
                assert_eq!(r, chunk.sample_rate, "VecSink: mixed rates in one stream");
                assert_eq!(c, chunk.channels, "VecSink: mixed channels in one stream");
            }
            _ => {
                self.sample_rate = Some(chunk.sample_rate);
                self.channels = Some(chunk.channels);
            }
        }
        self.samples.extend_from_slice(&chunk.frames);
        Ok(())
    }

    fn play(&mut self) -> Result<(), MusicError> {
        self.state = SinkState::Playing;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), MusicError> {
        self.state = SinkState::Paused;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MusicError> {
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn state(&self) -> SinkState {
        self.state
    }

    fn preferred_sample_rate(&self) -> Option<u32> {
        self.demand_rate
    }

    fn buffered_frames(&self) -> u64 {
        self.buffered
    }

    fn drain(&mut self) {
        self.drains += 1;
    }
}

// ---------------------------------------------------------------------------
// SinkRouter: per-track PCM / DoP path selection
// ---------------------------------------------------------------------------

/// Routes the engine's single [`AudioSink`] handle to one of two devices:
/// the shared-mode PCM sink (with the DSP chain) or the exclusive DoP
/// sink (bit-perfect). The engine calls [`select_output_path`](AudioSink::select_output_path)
/// on every track open; the router delegates everything else to the active
/// sink. Lives in kahawai-player-core (no platform imports) so the engine and its
/// tests stay portable; the real sinks live in `kahawai-player-audio`.
pub struct SinkRouter {
    pcm: Box<dyn AudioSink>,
    dop: Option<Box<dyn AudioSink>>,
    active: OutputPath,
}

impl SinkRouter {
    pub fn new(pcm: Box<dyn AudioSink>, dop: Option<Box<dyn AudioSink>>) -> Self {
        Self {
            pcm,
            dop,
            active: OutputPath::Pcm,
        }
    }

    fn active_sink(&mut self) -> &mut dyn AudioSink {
        match self.active {
            OutputPath::Pcm => &mut *self.pcm,
            OutputPath::Dop => match self.dop.as_mut() {
                Some(s) => &mut **s,
                None => &mut *self.pcm,
            },
        }
    }

    /// Which path is currently selected.
    pub fn active_path(&self) -> OutputPath {
        self.active
    }
}

impl AudioSink for SinkRouter {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        self.active_sink().open(track)
    }

    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        self.active_sink().write(chunk)
    }

    fn play(&mut self) -> Result<(), MusicError> {
        self.active_sink().play()
    }

    fn pause(&mut self) -> Result<(), MusicError> {
        self.active_sink().pause()
    }

    fn stop(&mut self) -> Result<(), MusicError> {
        // Stop both so a path switch never leaves the other device running.
        self.pcm.stop()?;
        if let Some(d) = self.dop.as_mut() {
            d.stop()?;
        }
        Ok(())
    }

    fn state(&self) -> SinkState {
        match self.active {
            OutputPath::Pcm => self.pcm.state(),
            OutputPath::Dop => self
                .dop
                .as_ref()
                .map(|d| d.state())
                .unwrap_or(SinkState::Stopped),
        }
    }

    fn latency_ms(&self) -> u64 {
        match self.active {
            OutputPath::Pcm => self.pcm.latency_ms(),
            OutputPath::Dop => self.dop.as_ref().map(|d| d.latency_ms()).unwrap_or(0),
        }
    }

    fn preferred_sample_rate(&self) -> Option<u32> {
        match self.active {
            OutputPath::Pcm => self.pcm.preferred_sample_rate(),
            OutputPath::Dop => None, // DoP rate is negotiated by the DoP sink itself
        }
    }

    fn supports_dop(&self) -> bool {
        self.dop.is_some()
    }

    fn dop_output_rate(&self, dsd_rate_hz: u32) -> Option<u32> {
        self.dop
            .as_ref()
            .and_then(|d| d.dop_output_rate(dsd_rate_hz))
    }

    fn select_output_path(&mut self, path: OutputPath) {
        // Never select a DoP path that doesn't exist.
        self.active = match path {
            OutputPath::Dop if self.dop.is_none() => OutputPath::Pcm,
            p => p,
        };
    }

    fn write_dop(&mut self, bytes: &[u8]) -> Result<(), MusicError> {
        match self.dop.as_mut() {
            Some(d) => d.write_dop(bytes),
            None => Err(MusicError::BadRequest("no DoP sink installed".into())),
        }
    }

    fn buffered_frames(&self) -> u64 {
        match self.active {
            OutputPath::Pcm => self.pcm.buffered_frames(),
            OutputPath::Dop => 0,
        }
    }

    fn drain(&mut self) {
        match self.active {
            OutputPath::Pcm => self.pcm.drain(),
            OutputPath::Dop => {
                if let Some(d) = self.dop.as_mut() {
                    d.drain();
                }
            }
        }
    }

    fn underrun_count(&self) -> u64 {
        match self.active {
            OutputPath::Pcm => self.pcm.underrun_count(),
            OutputPath::Dop => self.dop.as_ref().map(|d| d.underrun_count()).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kahawai_core::{format::AudioFormat, Track};

    fn track(id: i64) -> Track {
        Track {
            id,
            path: format!("/m/{id}.flac"),
            hash: String::new(),
            format: AudioFormat::Flac,
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            duration_ms: None,
            bitrate: None,
            title: None,
            album: None,
            artist: None,
            album_id: None,
            track_no: None,
            disc_no: None,
            genre: None,
            year: None,
            missing: false,
            decodable: true,
        }
    }

    #[test]
    fn null_sink_state_machine() {
        let mut sink = NullSink::new();
        assert_eq!(sink.state(), SinkState::Stopped);

        sink.open(&track(7)).unwrap();
        assert_eq!(sink.opened(), &[7]);

        sink.write(PcmChunk {
            frames: vec![0.0; 2048], // 1024 stereo frames
            sample_rate: 44100,
            channels: 2,
        })
        .unwrap();
        assert_eq!(sink.chunks_written, 1);
        assert_eq!(sink.frames_written, 1024);

        sink.play().unwrap();
        assert_eq!(sink.state(), SinkState::Playing);
        sink.pause().unwrap();
        assert_eq!(sink.state(), SinkState::Paused);
        sink.stop().unwrap();
        assert_eq!(sink.state(), SinkState::Stopped);
    }

    #[test]
    fn null_sink_records_open_order_for_gapless() {
        let mut sink = NullSink::new();
        for id in [1, 2, 3] {
            sink.open(&track(id)).unwrap();
        }
        assert_eq!(sink.opened(), &[1, 2, 3]);
    }

    /// Minimal DoP-capable test sink.
    struct FakeDopSink {
        pub bytes: Vec<u8>,
        state: SinkState,
    }

    impl AudioSink for FakeDopSink {
        fn open(&mut self, _track: &Track) -> Result<(), MusicError> {
            self.state = SinkState::Stopped;
            Ok(())
        }
        fn write(&mut self, _chunk: PcmChunk) -> Result<(), MusicError> {
            Err(MusicError::BadRequest("PCM write on a DoP sink".into()))
        }
        fn play(&mut self) -> Result<(), MusicError> {
            self.state = SinkState::Playing;
            Ok(())
        }
        fn pause(&mut self) -> Result<(), MusicError> {
            self.state = SinkState::Paused;
            Ok(())
        }
        fn stop(&mut self) -> Result<(), MusicError> {
            self.state = SinkState::Stopped;
            Ok(())
        }
        fn state(&self) -> SinkState {
            self.state
        }
        fn supports_dop(&self) -> bool {
            true
        }
        fn dop_output_rate(&self, dsd_rate_hz: u32) -> Option<u32> {
            crate::dop::dop_pcm_rate(dsd_rate_hz)
        }
        fn write_dop(&mut self, bytes: &[u8]) -> Result<(), MusicError> {
            self.bytes.extend_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn router_selects_paths_and_reports_dop() {
        let mut router = SinkRouter::new(
            Box::new(NullSink::new()),
            Some(Box::new(FakeDopSink {
                bytes: Vec::new(),
                state: SinkState::Stopped,
            })),
        );
        assert_eq!(router.active_path(), OutputPath::Pcm);
        assert!(router.supports_dop());
        assert_eq!(router.dop_output_rate(2_822_400), Some(176_400));
        assert_eq!(router.dop_output_rate(1234), None);

        router.select_output_path(OutputPath::Dop);
        assert_eq!(router.active_path(), OutputPath::Dop);
        router.open(&track(1)).unwrap();
        router.write_dop(&[1, 2, 3]).unwrap();
        // PCM writes on the DoP path go to the DoP sink and fail loudly.
        assert!(router
            .write(PcmChunk {
                frames: vec![0.0],
                sample_rate: 44100,
                channels: 1,
            })
            .is_err());

        router.select_output_path(OutputPath::Pcm);
        router
            .write(PcmChunk {
                frames: vec![0.0, 0.0],
                sample_rate: 44100,
                channels: 2,
            })
            .unwrap();
    }

    #[test]
    fn router_without_dop_sink_never_selects_dop() {
        let mut router = SinkRouter::new(Box::new(NullSink::new()), None);
        assert!(!router.supports_dop());
        router.select_output_path(OutputPath::Dop);
        assert_eq!(router.active_path(), OutputPath::Pcm);
        assert!(router.write_dop(&[1]).is_err());
    }
}
