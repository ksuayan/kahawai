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
/// DSP chain (EQ → loudness → volume). DoP and exclusive PCM both use the
/// exclusive hog-mode device and bypass all PCM DSP, bit-perfect: DoP carries
/// DSD, `PcmExclusive` carries ordinary PCM untouched (see `bitperfect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputPath {
    #[default]
    #[serde(rename = "pcm-shared")]
    Pcm,
    #[serde(rename = "dop-exclusive")]
    Dop,
    #[serde(rename = "pcm-exclusive")]
    PcmExclusive,
}

impl OutputPath {
    /// Both exclusive paths run on the same hog-mode device.
    pub fn is_exclusive(self) -> bool {
        matches!(self, OutputPath::Dop | OutputPath::PcmExclusive)
    }
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

    /// Choose the output device by name (`None` = the system default).
    /// Takes effect on the next `open`; a sink that holds a device (open
    /// stream, exclusive hog) must release it here. Sinks without a device
    /// choice ignore this. A name that is no longer connected falls back to
    /// the system default rather than failing playback.
    fn set_output_device(&mut self, _name: Option<&str>) {}

    /// Name of the device this sink would actually open (the chosen one, or
    /// what the system default resolves to), for matching against known
    /// DSD-capable DACs. `None` when unknown.
    fn output_device_name(&self) -> Option<String> {
        None
    }

    /// Is the output an external DAC (USB / Thunderbolt / FireWire) rather
    /// than built-in speakers, Bluetooth, AirPlay or a virtual device? Best
    /// quality only takes exclusive control of a device like that: hogging
    /// the built-in output would silence system sounds for no benefit.
    fn output_is_external_dac(&self) -> bool {
        false
    }

    /// The exact rate the sink can output *exclusively* for ordinary PCM
    /// (bit-perfect mode), or `None` when it cannot — the engine then falls
    /// back to shared-mode output. Capability query only: no device changes.
    fn exclusive_pcm_rate(&self, _rate_hz: u32) -> Option<u32> {
        None
    }

    /// Open the exclusive device for untouched PCM at `rate_hz` /
    /// `channels`. Samples are then pushed with [`write_dop`]
    /// (packed little-endian 24-bit frames); the device is released by
    /// `stop` (or the next `open*`).
    ///
    /// [`write_dop`]: AudioSink::write_dop
    fn open_exclusive_pcm(&mut self, _rate_hz: u32, _channels: u16) -> Result<(), MusicError> {
        Err(MusicError::BadRequest(
            "exclusive PCM output not supported by this sink".into(),
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
    /// Last device chosen via `set_output_device` (`None` = default).
    pub device: Option<String>,
    /// Sample rates this sink pretends to output exclusively (bit-perfect).
    pub exclusive_rates: Vec<u32>,
    /// Pretend the output is an external DAC (Best quality goes exclusive).
    pub external: bool,
    /// `(rate, channels)` of the currently open exclusive PCM stream.
    pub exclusive_open: Option<(u32, u16)>,
    /// Every packed 24-bit byte pushed through `write_dop`.
    pub exclusive_bytes: Vec<u8>,
    /// Output path last selected by the engine.
    pub selected_path: Option<OutputPath>,
    /// Number of times `open` (the shared path) was called.
    pub shared_opens: u32,
    /// Make `open_exclusive_pcm` fail (device refused / busy).
    pub fail_exclusive_open: bool,
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
        self.shared_opens += 1;
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

    fn set_output_device(&mut self, name: Option<&str>) {
        self.device = name.map(str::to_owned);
    }

    fn exclusive_pcm_rate(&self, rate_hz: u32) -> Option<u32> {
        self.exclusive_rates.contains(&rate_hz).then_some(rate_hz)
    }

    fn output_is_external_dac(&self) -> bool {
        self.external
    }

    fn open_exclusive_pcm(&mut self, rate_hz: u32, channels: u16) -> Result<(), MusicError> {
        if self.fail_exclusive_open {
            return Err(MusicError::Audio("device busy".into()));
        }
        self.exclusive_open = Some((rate_hz, channels));
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn write_dop(&mut self, bytes: &[u8]) -> Result<(), MusicError> {
        if self.exclusive_open.is_none() {
            return Err(MusicError::Audio("exclusive stream not open".into()));
        }
        self.exclusive_bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn select_output_path(&mut self, path: OutputPath) {
        self.selected_path = Some(path);
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
            OutputPath::Dop | OutputPath::PcmExclusive => match self.dop.as_mut() {
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
            OutputPath::Dop | OutputPath::PcmExclusive => self
                .dop
                .as_ref()
                .map(|d| d.state())
                .unwrap_or(SinkState::Stopped),
        }
    }

    fn latency_ms(&self) -> u64 {
        match self.active {
            OutputPath::Pcm => self.pcm.latency_ms(),
            OutputPath::Dop | OutputPath::PcmExclusive => {
                self.dop.as_ref().map(|d| d.latency_ms()).unwrap_or(0)
            }
        }
    }

    fn preferred_sample_rate(&self) -> Option<u32> {
        match self.active {
            OutputPath::Pcm => self.pcm.preferred_sample_rate(),
            // The exclusive sink negotiates its own rate.
            OutputPath::Dop | OutputPath::PcmExclusive => None,
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
        // Never select an exclusive path that doesn't exist.
        let next = match path {
            OutputPath::Dop | OutputPath::PcmExclusive if self.dop.is_none() => OutputPath::Pcm,
            p => p,
        };
        // The exclusive sink keeps its device hogged (and streaming) between
        // tracks so consecutive DSD tracks and seeks stay seamless. Going back
        // to shared PCM must give the device back.
        if next == OutputPath::Pcm && self.active != OutputPath::Pcm {
            if let Some(d) = self.dop.as_mut() {
                let _ = d.stop();
            }
        }
        self.active = next;
    }

    fn write_dop(&mut self, bytes: &[u8]) -> Result<(), MusicError> {
        match self.dop.as_mut() {
            Some(d) => d.write_dop(bytes),
            None => Err(MusicError::BadRequest("no exclusive sink installed".into())),
        }
    }

    fn exclusive_pcm_rate(&self, rate_hz: u32) -> Option<u32> {
        self.dop
            .as_ref()
            .and_then(|d| d.exclusive_pcm_rate(rate_hz))
    }

    fn open_exclusive_pcm(&mut self, rate_hz: u32, channels: u16) -> Result<(), MusicError> {
        match self.dop.as_mut() {
            Some(d) => d.open_exclusive_pcm(rate_hz, channels),
            None => Err(MusicError::BadRequest("no exclusive sink installed".into())),
        }
    }

    fn output_is_external_dac(&self) -> bool {
        self.dop
            .as_ref()
            .map(|d| d.output_is_external_dac())
            .unwrap_or(false)
    }

    fn output_device_name(&self) -> Option<String> {
        self.dop
            .as_ref()
            .and_then(|d| d.output_device_name())
            .or_else(|| self.pcm.output_device_name())
    }

    fn set_output_device(&mut self, name: Option<&str>) {
        // One choice for both paths: DSD/DoP must land on the device the
        // user picked, not silently on the system default.
        self.pcm.set_output_device(name);
        if let Some(d) = self.dop.as_mut() {
            d.set_output_device(name);
        }
    }

    fn buffered_frames(&self) -> u64 {
        match self.active {
            OutputPath::Pcm => self.pcm.buffered_frames(),
            OutputPath::Dop | OutputPath::PcmExclusive => {
                self.dop.as_ref().map(|d| d.buffered_frames()).unwrap_or(0)
            }
        }
    }

    fn drain(&mut self) {
        match self.active {
            OutputPath::Pcm => self.pcm.drain(),
            OutputPath::Dop | OutputPath::PcmExclusive => {
                if let Some(d) = self.dop.as_mut() {
                    d.drain();
                }
            }
        }
    }

    fn underrun_count(&self) -> u64 {
        match self.active {
            OutputPath::Pcm => self.pcm.underrun_count(),
            OutputPath::Dop | OutputPath::PcmExclusive => {
                self.dop.as_ref().map(|d| d.underrun_count()).unwrap_or(0)
            }
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
            mqa: false,
            original_sample_rate: None,
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
    fn leaving_an_exclusive_path_for_shared_pcm_releases_the_exclusive_sink() {
        let mut router = SinkRouter::new(
            Box::new(NullSink::new()),
            Some(Box::new(FakeDopSink {
                bytes: Vec::new(),
                state: SinkState::Stopped,
            })),
        );
        router.select_output_path(OutputPath::Dop);
        router.open(&track(1)).unwrap();
        router.play().unwrap();
        assert_eq!(router.state(), SinkState::Playing);
        // Staying on the exclusive path keeps it running (seamless handoff).
        router.select_output_path(OutputPath::Dop);
        assert_eq!(router.state(), SinkState::Playing);
        // Back to shared PCM: the exclusive device is given back.
        router.select_output_path(OutputPath::Pcm);
        router.select_output_path(OutputPath::Dop);
        assert_eq!(router.state(), SinkState::Stopped);
    }

    #[test]
    fn router_without_dop_sink_never_selects_dop() {
        let mut router = SinkRouter::new(Box::new(NullSink::new()), None);
        assert!(!router.supports_dop());
        router.select_output_path(OutputPath::Dop);
        assert_eq!(router.active_path(), OutputPath::Pcm);
        assert!(router.write_dop(&[1]).is_err());
    }

    // -- router: the two exclusive paths share one device ------------------

    /// Records what the router forwards, observable after the router owns it.
    #[derive(Default)]
    struct Log {
        ops: Vec<String>,
        exclusive_rates: Vec<u32>,
        buffered: u64,
    }

    #[derive(Clone, Default)]
    struct Recorder(std::sync::Arc<std::sync::Mutex<Log>>, &'static str);

    impl Recorder {
        fn ops(&self) -> Vec<String> {
            self.0.lock().unwrap().ops.clone()
        }
        fn log(&self, op: &str) {
            self.0.lock().unwrap().ops.push(format!("{}:{op}", self.1));
        }
    }

    impl AudioSink for Recorder {
        fn open(&mut self, _t: &Track) -> Result<(), MusicError> {
            self.log("open");
            Ok(())
        }
        fn write(&mut self, _c: PcmChunk) -> Result<(), MusicError> {
            self.log("write");
            Ok(())
        }
        fn play(&mut self) -> Result<(), MusicError> {
            self.log("play");
            Ok(())
        }
        fn pause(&mut self) -> Result<(), MusicError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), MusicError> {
            Ok(())
        }
        fn state(&self) -> SinkState {
            SinkState::Stopped
        }
        fn supports_dop(&self) -> bool {
            self.1 == "excl"
        }
        fn exclusive_pcm_rate(&self, r: u32) -> Option<u32> {
            self.0
                .lock()
                .unwrap()
                .exclusive_rates
                .contains(&r)
                .then_some(r)
        }
        fn open_exclusive_pcm(&mut self, r: u32, c: u16) -> Result<(), MusicError> {
            self.log(&format!("open_exclusive_pcm({r},{c})"));
            Ok(())
        }
        fn write_dop(&mut self, b: &[u8]) -> Result<(), MusicError> {
            self.log(&format!("write_dop({})", b.len()));
            Ok(())
        }
        fn buffered_frames(&self) -> u64 {
            self.0.lock().unwrap().buffered
        }
        fn drain(&mut self) {
            self.log("drain");
        }
    }

    fn router() -> (SinkRouter, Recorder, Recorder) {
        let pcm = Recorder(Default::default(), "pcm");
        let excl = Recorder(Default::default(), "excl");
        excl.0.lock().unwrap().exclusive_rates = vec![44100, 96000];
        excl.0.lock().unwrap().buffered = 1234;
        let r = SinkRouter::new(Box::new(pcm.clone()), Some(Box::new(excl.clone())));
        (r, pcm, excl)
    }

    #[test]
    fn exclusive_pcm_is_routed_to_the_exclusive_sink_not_the_shared_one() {
        let (mut r, pcm, excl) = router();
        r.select_output_path(OutputPath::PcmExclusive);
        assert_eq!(r.active_path(), OutputPath::PcmExclusive);
        assert_eq!(r.exclusive_pcm_rate(96000), Some(96000));
        assert_eq!(r.exclusive_pcm_rate(48000), None);
        r.open_exclusive_pcm(96000, 2).unwrap();
        r.play().unwrap();
        r.write_dop(&[0u8; 12]).unwrap();
        r.drain();
        assert_eq!(
            excl.ops(),
            vec![
                "excl:open_exclusive_pcm(96000,2)",
                "excl:play",
                "excl:write_dop(12)",
                "excl:drain"
            ]
        );
        assert!(pcm.ops().is_empty(), "the shared sink was not touched");
    }

    #[test]
    fn the_exclusive_buffer_is_what_position_uses_on_both_exclusive_paths() {
        let (mut r, _p, _e) = router();
        r.select_output_path(OutputPath::Pcm);
        assert_eq!(r.buffered_frames(), 0);
        for path in [OutputPath::Dop, OutputPath::PcmExclusive] {
            r.select_output_path(path);
            assert_eq!(r.buffered_frames(), 1234, "{path:?}");
        }
    }

    #[test]
    fn without_an_exclusive_sink_the_exclusive_paths_are_refused() {
        let pcm = Recorder(Default::default(), "pcm");
        let mut r = SinkRouter::new(Box::new(pcm.clone()), None);
        r.select_output_path(OutputPath::PcmExclusive);
        assert_eq!(
            r.active_path(),
            OutputPath::Pcm,
            "falls back to the shared path"
        );
        assert_eq!(r.exclusive_pcm_rate(44100), None);
        assert!(r.open_exclusive_pcm(44100, 2).is_err());
        assert!(r.write_dop(&[0; 6]).is_err());
    }

    #[test]
    fn sinks_that_do_not_opt_in_refuse_exclusive_pcm() {
        let mut n = NullSink::new();
        assert_eq!(n.exclusive_pcm_rate(44100), None);
        assert!(n.open_exclusive_pcm(44100, 2).is_err());
    }

    #[test]
    fn output_path_wire_names_and_helpers() {
        assert_eq!(
            serde_json::to_string(&OutputPath::Pcm).unwrap(),
            "\"pcm-shared\""
        );
        assert_eq!(
            serde_json::to_string(&OutputPath::Dop).unwrap(),
            "\"dop-exclusive\""
        );
        assert_eq!(
            serde_json::to_string(&OutputPath::PcmExclusive).unwrap(),
            "\"pcm-exclusive\""
        );
        assert!(!OutputPath::Pcm.is_exclusive());
        assert!(OutputPath::Dop.is_exclusive() && OutputPath::PcmExclusive.is_exclusive());
    }
}
