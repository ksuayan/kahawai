//! Playback engine: state machine, decode pump, gapless advance, seek.
//! (Spec: kahawai-player-design.md "Player core & audio engine".)
//!
//! Threading contract (C1): a [`Player`] lives on exactly one dedicated
//! playback thread owned by [`EngineController`]. That thread decodes with
//! symphonia and calls `AudioSink::write` — it is *not* a real-time audio
//! callback thread. Sinks must keep `write()` light: C2's real sink hands
//! the samples to its own RT callback through a ring buffer *inside*
//! `write()`, so decode never runs on the audio thread.
//!
//! Gapless (S8): the engine opens each track with `?next=<id>` and honors
//! the server's `X-Gapless-Mode`. `single-session` decodes straight to EOF
//! (gapless by construction); `chained` continues into a re-probed decoder
//! with no dropped/added samples; when chaining fails or is unavailable
//! the engine falls back to sequential requests — still sample-accurate,
//! just a fresh HTTP round-trip between tracks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kahawai_core::{
    api::StreamFormat,
    format::{transcode_ladder, AudioFormat},
    MusicError, Track,
};
use serde::{Deserialize, Serialize};

use crate::bitperfect::{f32_to_i24_le, BitPerfect};
use crate::decode::{DecodedSpec, StreamDecoder};
use crate::dop::{dop_pcm_rate, DopSpec, DopStream};
use crate::analog::{AnalogSettings, AnalogStage};
use crate::dsp::{DspStage, 
    scan_track_lufs, EqBand, GainRamp, LoudnessNorm, ParametricEq, DEFAULT_LOUDNESS_TARGET,
};
use crate::queue::{Queue, RepeatMode};
use crate::resample::CubicResampler;
use crate::sink::{AudioSink, OutputPath, PcmChunk};
use crate::transport::{HttpTransport, StreamOptions, StreamProgress, Transport};

/// Frames decoded per pump iteration (~93 ms at 44.1 kHz).
const CHUNK_FRAMES: usize = 4096;

/// Milliseconds of playback that count as "restart the track" for prev.
const PREV_RESTART_MS: u64 = 3000;

/// Default server URL when no setting is stored.
pub const DEFAULT_SERVER_URL: &str = "http://localhost:8080";

// ---------------------------------------------------------------------------
// Format selection (S13)
// ---------------------------------------------------------------------------

/// What to do with DSD (DSF/DFF) tracks when no explicit format override
/// applies. The server cannot know the client's DAC, so this is a client
/// preference (Settings → DSD handling): `Native` requests DoP (bit-perfect
/// to a DSD-capable DAC, falls back to FLAC when the device can't take the
/// DoP rate); `Convert` transcodes to PCM/FLAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DsdStory {
    Native,
    #[default]
    Convert,
}

/// Resolve the rendition for a track: per-track override wins, then the
/// global preference, then the DSD story (native DoP for DSD tracks when
/// the user opted in), then the desktop ladder (passthrough when the source
/// is directly streamable, else FLAC). Mirrors the server's default ladder;
/// an explicit choice is always honored as-is (including `dop`).
pub fn resolve_format(
    track: &Track,
    global: Option<StreamFormat>,
    per_track: Option<StreamFormat>,
    dsd_story: DsdStory,
) -> StreamFormat {
    if let Some(f) = per_track {
        return f;
    }
    if let Some(f) = global {
        return f;
    }
    if dsd_story == DsdStory::Native && matches!(track.format, AudioFormat::Dsf | AudioFormat::Dff)
    {
        return StreamFormat::Dop;
    }
    transcode_ladder(
        track.format,
        &[StreamFormat::Passthrough, StreamFormat::Flac],
    )
}

/// Renditions the format picker may offer for a track. Empty = not
/// playable in v1 (SACD ISO needs offline extraction first).
pub fn valid_formats(track: &Track) -> Vec<StreamFormat> {
    match track.format {
        AudioFormat::SacdIso | AudioFormat::Unknown => vec![],
        AudioFormat::Dsf | AudioFormat::Dff => vec![StreamFormat::Flac, StreamFormat::Dop],
        _ if track.format.is_directly_streamable() => vec![
            StreamFormat::Passthrough,
            StreamFormat::Flac,
            StreamFormat::Opus,
            StreamFormat::Mp3,
        ],
        _ => vec![StreamFormat::Flac],
    }
}

// ---------------------------------------------------------------------------
// Snapshot / events
// ---------------------------------------------------------------------------

/// Player state as seen by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerStatus {
    #[default]
    Stopped,
    Loading,
    Playing,
    Paused,
}

/// Serializable snapshot pushed to the UI (~4 Hz while playing, immediately
/// on track/state changes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub status: PlayerStatus,
    pub track: Option<Track>,
    /// Queue track ids in list order (not shuffle order).
    pub queue_ids: Vec<i64>,
    /// Index into `queue_ids` of the current track; `None` when the queue
    /// is empty or nothing has played yet.
    pub queue_index: Option<usize>,
    /// Id of the currently playing track, if any.
    pub current_id: Option<i64>,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    /// How far into the track the data received from the server reaches
    /// (ms). `None` when the server didn't say how big the stream is
    /// (transcoded/chunked) or several tracks share one response.
    #[serde(default)]
    pub buffered_ms: Option<u64>,
    /// Sample rate of the audio reaching the output (after any resampling);
    /// the rate the EQ is designed at. `None` when idle.
    #[serde(default)]
    pub output_rate_hz: Option<u32>,
    /// What the analog stage is doing, e.g. "4x oversampling + ADAA, 0.7 ms
    /// latency"; `None` when it is off or nothing is playing on the shared path.
    #[serde(default)]
    pub analog_plan: Option<String>,
    /// The rendition actually streaming (explicit `?format=` value).
    pub format: Option<StreamFormat>,
    /// `X-Transcode-Chain` of the active response.
    pub chain: Option<String>,
    /// Active output path: `pcm-shared` (DSP chain live) or
    /// `dop-exclusive` (hog-mode DoP, DSP bypassed).
    pub output_path: OutputPath,
    pub volume: f32,
    pub error: Option<String>,
    /// Playback modes, mirrored to the UI so it never invents them.
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            status: PlayerStatus::Stopped,
            track: None,
            queue_ids: Vec::new(),
            queue_index: None,
            current_id: None,
            position_ms: 0,
            duration_ms: None,
            buffered_ms: None,
            output_rate_hz: None,
            analog_plan: None,
            format: None,
            chain: None,
            output_path: OutputPath::Pcm,
            volume: 1.0,
            error: None,
            repeat: RepeatMode::Off,
            shuffle: false,
        }
    }
}

/// Serializable DSP settings: persisted by the shell (engine-settings.json)
/// and mirrored by the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DspSettings {
    pub eq_bands: Vec<EqBand>,
    pub eq_enabled: bool,
    pub loudness_enabled: bool,
    pub loudness_target: f32,
    /// Analog character stage (tube / transistor warmth). Older settings
    /// files have none: it defaults to off.
    #[serde(default)]
    pub analog: AnalogSettings,
}

impl Default for DspSettings {
    fn default() -> Self {
        Self {
            eq_bands: Vec::new(),
            eq_enabled: true,
            loudness_enabled: false,
            loudness_target: DEFAULT_LOUDNESS_TARGET,
            analog: AnalogSettings::default(),
        }
    }
}

/// Events the Tauri shell forwards to the frontend.
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    State(PlayerSnapshot),
}

// ---------------------------------------------------------------------------
// Active stream
// ---------------------------------------------------------------------------

/// One open `/stream/:id` PCM response being pumped to the sink. In
/// chained / single-session gapless mode the response carries the next
/// track's audio too; `segments` tracks which track is *displayed* (the
/// audio itself is always continuous).
struct PcmStream {
    segments: Vec<Track>,
    /// Index into `segments` of the displayed track.
    seg_idx: usize,
    /// Expected frames per segment, from catalog `duration_ms` — used only
    /// to move the *displayed* track across an invisible server-side
    /// boundary. The audio is gapless regardless of this estimate.
    seg_frames: Vec<u64>,
    /// `X-Gapless-Mode` when the server chained audio into this response.
    gapless_mode: Option<String>,
    format_used: StreamFormat,
    chain: Option<String>,
    decoder: StreamDecoder,
    spec: DecodedSpec,
    resampler: Option<CubicResampler>,
    /// Rate of the frames handed to the sink (post-resample).
    sink_rate: u32,
    /// Decoded frames (pre-resample) consumed from the response.
    decoded_frames: u64,
    /// Frames written to the sink (post-resample).
    pumped_frames: u64,
    /// Position offset in sink-rate frames (transcode `?seek_ms=`).
    base_frames: u64,
    /// Passthrough seek: decode-and-drop this many frames before writing.
    skip_frames: u64,
    /// Exclusive bit-perfect output: samples go to the sink untouched as
    /// packed 24-bit, bypassing EQ, loudness, volume and resampling.
    bit_perfect: bool,
    /// Bytes received from the network (see `ActiveStream::buffered_ms`).
    progress: Option<StreamProgress>,
}

/// One open `?format=dop` response. DoP bytes flow to the sink untouched —
/// no decode, no DSP, no volume, no resample.
struct DopPlayback {
    segments: Vec<Track>,
    seg_idx: usize,
    seg_frames: Vec<u64>,
    gapless_mode: Option<String>,
    chain: Option<String>,
    stream: DopStream,
    spec: DopSpec,
    /// DoP frames written to the sink.
    pumped_frames: u64,
    /// Position offset in DoP frames (`?seek_ms=`).
    base_frames: u64,
    progress: Option<StreamProgress>,
}

/// The engine's active response: PCM (decoded, DSP'd) or DoP (byte pipe).
enum ActiveStream {
    Pcm(PcmStream),
    Dop(DopPlayback),
}

impl PcmStream {
    /// Move the displayed track across the estimated segment boundary.
    fn advance_display(&mut self) {
        while self.seg_idx + 1 < self.segments.len() {
            let boundary: u64 = self.seg_frames[..=self.seg_idx].iter().sum();
            if self.decoded_frames >= boundary {
                self.seg_idx += 1;
            } else {
                break;
            }
        }
    }
}

impl DopPlayback {
    /// Move the displayed track across the estimated segment boundary.
    fn advance_display(&mut self) {
        while self.seg_idx + 1 < self.segments.len() {
            let boundary: u64 = self.seg_frames[..=self.seg_idx].iter().sum();
            if self.pumped_frames >= boundary {
                self.seg_idx += 1;
            } else {
                break;
            }
        }
    }
}

impl ActiveStream {
    /// Playhead in ms. `buffered` = frames the sink has accepted but not
    /// played yet (PCM path): the position is what is audible, not what has
    /// merely been decoded and queued.
    fn position_ms(&self, buffered: u64) -> u64 {
        match self {
            ActiveStream::Pcm(a) => {
                let played = a.pumped_frames.saturating_sub(buffered);
                (a.base_frames + played) * 1000 / a.sink_rate as u64
            }
            ActiveStream::Dop(a) => {
                (a.base_frames + a.pumped_frames) * 1000 / a.spec.dop_rate_hz as u64
            }
        }
    }

    /// How far into the displayed track the bytes received so far reach, in
    /// ms; `None` when unknowable (chunked/transcoded body, or a gapless
    /// response carrying several tracks). Never behind the playhead.
    fn buffered_ms(&self, position_ms: u64) -> Option<u64> {
        let (progress, segments, base_ms) = match self {
            ActiveStream::Pcm(a) => (
                a.progress.as_ref()?,
                &a.segments,
                a.base_frames * 1000 / a.sink_rate as u64,
            ),
            ActiveStream::Dop(a) => (
                a.progress.as_ref()?,
                &a.segments,
                a.base_frames * 1000 / a.spec.dop_rate_hz as u64,
            ),
        };
        if segments.len() != 1 {
            return None;
        }
        let duration = segments[0].duration_ms?;
        let frac = progress.fraction()?;
        let ms = if progress.offset > 0 {
            (duration as f64 * frac) as u64
        } else {
            base_ms + ((duration.saturating_sub(base_ms)) as f64 * frac) as u64
        };
        Some(ms.clamp(position_ms.min(duration), duration))
    }

    /// Sample rate of the audio the sink is receiving.
    fn output_rate_hz(&self) -> u32 {
        match self {
            ActiveStream::Pcm(a) => a.sink_rate,
            ActiveStream::Dop(a) => a.spec.dop_rate_hz,
        }
    }

    fn display_track(&self) -> &Track {
        match self {
            ActiveStream::Pcm(a) => &a.segments[a.seg_idx],
            ActiveStream::Dop(a) => &a.segments[a.seg_idx],
        }
    }

    fn format_used(&self) -> StreamFormat {
        match self {
            ActiveStream::Pcm(a) => a.format_used,
            ActiveStream::Dop(_) => StreamFormat::Dop,
        }
    }

    /// The processing chain of the track being *heard*. A gapless response
    /// carries several tracks and the server joins their chains with
    /// " + " ("dsf64->flac 24/88.2 + dsf64->flac 24/88.2"); show only the
    /// displayed segment's.
    fn chain(&self) -> Option<String> {
        let (chain, seg) = match self {
            ActiveStream::Pcm(a) => (a.chain.as_deref(), a.seg_idx),
            ActiveStream::Dop(a) => (a.chain.as_deref(), a.seg_idx),
        };
        chain.map(|c| chain_segment(c, seg).to_string())
    }

    /// How many queue positions this response's audio consumed. The
    /// response always starts at the queue cursor.
    fn tracks_consumed(&self) -> usize {
        match self {
            ActiveStream::Pcm(a) => match a.gapless_mode.as_deref() {
                // One container stream holds every segment by construction.
                Some("single-session") => a.segments.len(),
                // One container stream per segment; a broken chain falls back
                // to sequential requests for whatever didn't play.
                Some("chained") => a.decoder.streams_completed.min(a.segments.len()).max(1),
                _ => 1,
            },
            ActiveStream::Dop(a) => match a.gapless_mode.as_deref() {
                Some("chained") => a.stream.segments_completed.min(a.segments.len()).max(1),
                // DoP single-session is one WAV per response in practice;
                // treat like the PCM rule for uniformity.
                Some("single-session") => a.segments.len(),
                _ => 1,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Player: the state machine (single-threaded)
// ---------------------------------------------------------------------------

/// Playback state machine. All methods are synchronous and must be called
/// from the playback thread; [`EngineController`] enforces that.
pub struct Player {
    sink: Box<dyn AudioSink>,
    transport: Box<dyn Transport>,
    queue: Queue,
    status: PlayerStatus,
    global_format: Option<StreamFormat>,
    /// Client DSD preference (Settings → DSD handling); consulted by
    /// [`resolve_format`] when no explicit override applies.
    dsd_story: DsdStory,
    /// When to use the exclusive, untouched PCM path (see `bitperfect`).
    bit_perfect: BitPerfect,
    track_formats: HashMap<i64, StreamFormat>,
    volume: f32,
    active: Option<ActiveStream>,
    error: Option<String>,
    /// Where the queue (tracks + cursor + modes) is persisted. `None`
    /// disables persistence (some tests).
    queue_path: Option<PathBuf>,
    /// Where the restored queue was left inside its current track. Used once,
    /// by the first `resume`; any other way of opening a track discards it.
    resume_at_ms: Option<u64>,
    /// Consecutive responses that produced zero audio frames. Guards
    /// against an infinite skip loop on a poison track with repeat-all.
    empty_streak: u32,
    // -- v1 DSP (PCM only; the DoP path never touches these) --
    eq: ParametricEq,
    analog: AnalogStage,
    loudness: LoudnessNorm,
    /// Click-free gain transitions between tracks with different
    /// loudness gains (~50 ms linear ramp).
    gain_ramp: GainRamp,
    output_path: OutputPath,
    /// DoP format established by the last headered response; a seeked
    /// response carries raw frames and continues under this spec.
    dop_spec: Option<DopSpec>,
}

impl Player {
    pub fn new(sink: Box<dyn AudioSink>, transport: Box<dyn Transport>) -> Self {
        Self {
            sink,
            transport,
            queue: Queue::new(),
            status: PlayerStatus::Stopped,
            global_format: None,
            dsd_story: DsdStory::default(),
            bit_perfect: BitPerfect::default(),
            track_formats: HashMap::new(),
            volume: 1.0,
            active: None,
            error: None,
            queue_path: None,
            resume_at_ms: None,
            empty_streak: 0,
            eq: ParametricEq::new(44100),
            analog: AnalogStage::new(44100),
            loudness: LoudnessNorm::new(DEFAULT_LOUDNESS_TARGET),
            gain_ramp: GainRamp::new(2205),
            output_path: OutputPath::Pcm,
            dop_spec: None,
        }
    }

    // -- commands ----------------------------------------------------------

    /// Replace the queue and start playing at `index`.
    pub fn play_queue(&mut self, tracks: Vec<Track>, index: usize) {
        self.error = None;
        self.queue.set_tracks(tracks);
        if self.queue.is_empty() {
            self.stop();
            return;
        }
        // Clamp the index: set_tracks resets the cursor to 0; walk forward.
        for _ in 0..index.min(self.queue.len().saturating_sub(1)) {
            self.queue.next_track();
        }
        self.persist_queue();
        self.open_current(None);
    }

    /// Restore a persisted queue without starting playback (launch
    /// restore). The cursor lands on the persisted index; status stays
    /// Stopped so the UI can hydrate and the user presses play.
    pub fn restore_queue(
        &mut self,
        tracks: Vec<Track>,
        index: usize,
        repeat: RepeatMode,
        shuffle: bool,
        position_ms: u64,
    ) {
        self.error = None;
        self.active = None;
        let _ = self.sink.stop();
        self.queue.set_tracks(tracks);
        if self.queue.is_empty() {
            self.status = PlayerStatus::Stopped;
            return;
        }
        // Modes are set here, in memory, without persisting: writing the
        // file while the queue is still empty would erase what is being
        // restored. Shuffle order is deterministic, so the cursor lands on
        // the same track.
        self.queue.repeat = repeat;
        self.queue.set_shuffle(shuffle);
        for _ in 0..index.min(self.queue.len().saturating_sub(1)) {
            self.queue.next_track();
        }
        self.resume_at_ms = (position_ms > 0).then_some(position_ms);
        self.status = PlayerStatus::Stopped;
        self.persist_queue();
    }

    /// Point queue persistence at `path` (`queue.json` next to the engine
    /// settings file). `None` disables it.
    pub fn set_queue_path(&mut self, path: Option<PathBuf>) {
        self.queue_path = path;
    }

    /// Append tracks to the end of the queue without disturbing playback.
    /// ("Add to queue".)
    pub fn append_tracks(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        self.queue.append(tracks);
        self.persist_queue();
    }

    /// Insert tracks right after the current item in playback order without
    /// disturbing playback. ("Play next".)
    pub fn insert_tracks_next(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        self.queue.insert_after_current(tracks);
        self.persist_queue();
    }

    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.queue.repeat = mode;
        self.persist_queue();
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.queue.set_shuffle(on);
        self.persist_queue();
    }

    /// Choose when to play through the exclusive bit-perfect path. A track
    /// that is loaded is re-opened at its current position so the change
    /// applies immediately (paused stays paused).
    pub fn set_bit_perfect(&mut self, mode: BitPerfect) {
        if mode == self.bit_perfect {
            return;
        }
        self.bit_perfect = mode;
        let live = self.active.is_some()
            && matches!(self.status, PlayerStatus::Playing | PlayerStatus::Paused);
        if live {
            let pos = self.position_ms();
            self.seek_ms(pos);
        }
    }

    /// Would this track play through the bit-perfect path, judging by the
    /// preference and what the file is (the device is checked once the
    /// stream's real sample rate is known)?
    fn wants_bit_perfect(&self, track: &Track, fmt: StreamFormat) -> bool {
        use kahawai_core::format::AudioFormat as F;
        fmt == StreamFormat::Passthrough
            && self.bit_perfect.applies_to(track.mqa)
            && !matches!(track.format, F::Dsf | F::Dff | F::SacdIso | F::Unknown)
    }

    pub fn set_dsd_story(&mut self, story: DsdStory) {
        self.dsd_story = story;
    }

    /// Playhead worth remembering: the live one, or the restored one not yet
    /// resumed. A stopped player has none.
    fn saved_position_ms(&self) -> u64 {
        match self.status {
            PlayerStatus::Playing | PlayerStatus::Paused => self.position_ms(),
            _ => self.resume_at_ms.unwrap_or(0),
        }
    }

    /// Save the queue and playhead (called ~every 5 s while playing).
    pub fn persist_position(&self) {
        self.persist_queue();
    }

    /// Best-effort queue persistence: tracks + cursor + modes as JSON.
    /// Failures are logged, never fatal to playback.
    fn persist_queue(&self) {
        let Some(path) = self.queue_path.as_ref() else {
            return;
        };
        let pq = PersistedQueue {
            tracks: self.queue.ordered_tracks().to_vec(),
            index: self.queue.index().min(self.queue.len().saturating_sub(1)),
            repeat: self.queue.repeat,
            shuffle: self.queue.shuffle,
            position_ms: self.saved_position_ms(),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string(&pq) {
            Ok(text) => {
                if let Err(e) = std::fs::write(path, text) {
                    tracing::warn!("queue persistence failed: {e}");
                }
            }
            Err(e) => tracing::warn!("queue serialize failed: {e}"),
        }
    }

    pub fn pause(&mut self) {
        if self.status == PlayerStatus::Playing {
            self.status = PlayerStatus::Paused;
            let _ = self.sink.pause();
            self.persist_queue();
        }
    }

    pub fn resume(&mut self) {
        match self.status {
            PlayerStatus::Paused => {
                self.status = PlayerStatus::Playing;
                let _ = self.sink.play();
            }
            PlayerStatus::Stopped if self.queue.current().is_some() => {
                let at = self.resume_at_ms.take();
                self.open_current(at);
            }
            _ => {}
        }
    }

    pub fn toggle(&mut self) {
        match self.status {
            PlayerStatus::Playing => self.pause(),
            PlayerStatus::Paused | PlayerStatus::Stopped => self.resume(),
            PlayerStatus::Loading => {}
        }
    }

    pub fn stop(&mut self) {
        self.status = PlayerStatus::Stopped;
        self.active = None;
        self.resume_at_ms = None;
        let _ = self.sink.stop();
        self.persist_queue();
    }

    /// User-initiated next: abandon any chained audio and start the next
    /// queue item fresh.
    pub fn next(&mut self) {
        if self.queue.next_track().is_some() {
            let paused = self.status == PlayerStatus::Paused;
            self.persist_queue();
            self.open_current(None);
            if paused {
                self.status = PlayerStatus::Paused;
                let _ = self.sink.pause();
            }
        } else {
            self.persist_queue();
            self.stop();
        }
    }

    /// User-initiated prev: restart the track when a few seconds in,
    /// otherwise go back.
    pub fn prev(&mut self) {
        let pos = self.position_ms();
        if pos > PREV_RESTART_MS {
            self.seek_ms(0);
            return;
        }
        if self.queue.prev_track().is_some() {
            let paused = self.status == PlayerStatus::Paused;
            self.persist_queue();
            self.open_current(None);
            if paused {
                self.status = PlayerStatus::Paused;
                let _ = self.sink.pause();
            }
        }
    }

    /// Scrub (S12). Transcodes re-request with `?seek_ms=` (sample-exact on
    /// the server); passthrough restarts the byte stream and the engine
    /// skips decoded frames to the target (C1 limitation, documented).
    pub fn seek_ms(&mut self, ms: u64) {
        if self.queue.current().is_none() {
            return;
        }
        let paused = self.status == PlayerStatus::Paused;
        self.error = None;
        self.open_current(Some(ms));
        if paused && self.status == PlayerStatus::Playing {
            self.status = PlayerStatus::Paused;
            let _ = self.sink.pause();
        }
    }

    pub fn set_global_format(&mut self, fmt: Option<StreamFormat>) {
        self.global_format = fmt;
    }

    pub fn set_track_format(&mut self, track_id: i64, fmt: Option<StreamFormat>) {
        match fmt {
            Some(f) => {
                self.track_formats.insert(track_id, f);
            }
            None => {
                self.track_formats.remove(&track_id);
            }
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
    }

    /// Route output to a named device (`None` = system default). If a
    /// track is loaded the stream is re-opened on the new device at the
    /// current audible position, keeping paused/playing as it was.
    pub fn set_output_device(&mut self, name: Option<String>) {
        self.sink.set_output_device(name.as_deref());
        let live = self.active.is_some()
            && matches!(self.status, PlayerStatus::Playing | PlayerStatus::Paused);
        if live {
            let pos = self.position_ms();
            self.seek_ms(pos);
        }
    }

    // -- v1 DSP (PCM only; DoP bypasses all of it) --

    pub fn set_eq_bands(&mut self, bands: Vec<EqBand>) -> Result<(), MusicError> {
        self.eq.set_bands(bands)
    }

    pub fn set_eq_enabled(&mut self, enabled: bool) {
        self.eq.set_enabled(enabled);
    }

    /// Analog character stage (PCM shared path only; DoP and bit-perfect
    /// never call it).
    pub fn set_analog(&mut self, settings: AnalogSettings) {
        self.analog.set_settings(settings);
    }

    pub fn set_loudness_enabled(&mut self, enabled: bool) {
        self.loudness.set_enabled(enabled);
    }

    pub fn set_loudness_target(&mut self, lufs: f32) {
        self.loudness.set_target(lufs);
    }

    /// Current DSP settings, for the shell to persist and the UI to mirror.
    pub fn dsp_settings(&self) -> DspSettings {
        DspSettings {
            eq_bands: self.eq.bands().to_vec(),
            eq_enabled: self.eq.enabled(),
            loudness_enabled: self.loudness.enabled(),
            loudness_target: self.loudness.target(),
            analog: self.analog.settings(),
        }
    }

    /// True when the playback thread should call [`pump`](Self::pump).
    pub fn wants_pump(&self) -> bool {
        self.status == PlayerStatus::Playing
    }

    // -- streaming ---------------------------------------------------------

    /// Open the current queue item. `seek` = scrub target in ms.
    fn open_current(&mut self, seek: Option<u64>) {
        self.resume_at_ms = None; // a restored position only applies to the first resume
        let track = match self.queue.current().cloned() {
            Some(t) => t,
            None => {
                self.stop();
                return;
            }
        };
        self.status = PlayerStatus::Loading;
        self.error = None;

        let mut fmt = resolve_format(
            &track,
            self.global_format,
            self.track_formats.get(&track.id).copied(),
            self.dsd_story,
        );

        // DoP capability check: the device must take the exact DoP rate
        // for this track's DSD rate. Otherwise fall back to DSD→PCM/FLAC
        // per the dsd_story resolution — playback never fails outright
        // for a capable-of-PCM device.
        if fmt == StreamFormat::Dop {
            if !self.sink.supports_dop() {
                self.fail("DoP needs a DSD-capable sink");
                return;
            }
            if self.dop_capable_rate(&track).is_none() {
                tracing::warn!(
                    track_id = track.id,
                    "DoP rate not supported by the output device; falling back to FLAC"
                );
                fmt = StreamFormat::Flac;
            }
        }

        let path = if fmt == StreamFormat::Dop {
            OutputPath::Dop
        } else {
            OutputPath::Pcm
        };
        self.sink.select_output_path(path);
        self.output_path = path;

        if fmt == StreamFormat::Dop {
            self.open_dop(&track, seek);
        } else {
            self.open_pcm(&track, seek, fmt);
        }
    }

    /// DoP rate this track needs *and* the sink confirmed, or `None` when
    /// the track has no known DSD rate or the device refuses the rate.
    fn dop_capable_rate(&self, track: &Track) -> Option<u32> {
        let dsd_rate = track.sample_rate?;
        let dop_rate = dop_pcm_rate(dsd_rate)?;
        // The sink reports the rate it will actually output (the CoreAudio
        // sink verifies the device switched); it must equal the required
        // rate, or DoP would play garbage.
        match self.sink.dop_output_rate(dsd_rate) {
            Some(r) if r == dop_rate => Some(dop_rate),
            _ => None,
        }
    }

    /// Open a `?format=dop` response: parse the WAV header (fresh play) or
    /// continue raw frames under the established spec (seek), then hand
    /// the bytes to the DoP sink untouched. No DSP, no volume, no resample.
    fn open_dop(&mut self, track: &Track, seek: Option<u64>) {
        let dop_rate = match self.dop_capable_rate(track) {
            Some(r) => r,
            None => {
                self.fail("DoP rate not available for this track/device");
                return;
            }
        };
        let next_id = self.queue.peek_next().map(|t| t.id);
        let opts = StreamOptions {
            format: Some(StreamFormat::Dop),
            seek_ms: seek,
            next: next_id,
            range_start: None,
        };
        let info = match self.transport.open_stream(track.id, &opts) {
            Ok(i) => i,
            Err(e) => {
                self.fail(&format!("stream failed: {e}"));
                return;
            }
        };
        let gapless_mode = info.gapless_mode.clone();
        let progress = info.progress.clone();
        let chained = gapless_mode.as_deref() == Some("chained");
        let established = self.dop_spec;
        let stream = match DopStream::new(info.reader, dop_rate, established, chained) {
            Ok(s) => s,
            Err(e) => {
                self.fail(&format!("DoP stream failed: {e}"));
                return;
            }
        };
        let spec = stream.spec();
        // Remember the established format for seek continuations.
        self.dop_spec = Some(spec);

        // Segments are display-only; the byte stream is continuous.
        let mut segments = vec![track.clone()];
        let mut seg_frames = vec![expected_frames(track, spec.dop_rate_hz)];
        if gapless_mode.is_some() {
            if let Some(next) = self.queue.peek_next().cloned() {
                // Sanity: the chained id must be the one we asked for.
                if Some(next.id) == info.gapless_next.or(next_id) {
                    seg_frames.push(expected_frames(&next, spec.dop_rate_hz));
                    segments.push(next);
                }
            }
        }

        if self.sink.open(track).is_err() || self.sink.play().is_err() {
            self.fail("DoP sink failed to open");
            return;
        }

        let base_frames = seek.unwrap_or(0) * spec.dop_rate_hz as u64 / 1000;
        self.active = Some(ActiveStream::Dop(DopPlayback {
            segments,
            seg_idx: 0,
            seg_frames,
            gapless_mode,
            chain: info.chain,
            stream,
            spec,
            pumped_frames: 0,
            base_frames,
            progress,
        }));
        self.status = PlayerStatus::Playing;
    }

    fn open_pcm(&mut self, track: &Track, seek: Option<u64>, fmt: StreamFormat) {
        debug_assert_ne!(fmt, StreamFormat::Dop);
        let want_bp = self.wants_bit_perfect(track, fmt);
        // Bit-perfect plays one track per stream: the exclusive device is
        // opened at each file's own rate, so the server must not chain the
        // next track (whose rate may differ) into this response.
        let next_id = if want_bp {
            None
        } else {
            self.queue.peek_next().map(|t| t.id)
        };

        // Loudness pre-scan (v1): one extra deterministic stream per
        // first-play of a track; the gain is cached by (track, format).
        // Cost: double LAN bandwidth + a second server transcode for
        // uncached tracks. DoP never reaches this path.
        let loudness_gain_db = if self.loudness.enabled() && !want_bp {
            let track_id = track.id;
            let transport = &*self.transport;
            self.loudness
                .gain_for(track_id, fmt, || scan_track_lufs(transport, track_id, fmt))
        } else {
            0.0
        };
        self.gain_ramp.retarget(loudness_gain_db);

        // Passthrough seeks use Range + decode-skip; everything else uses
        // the server's sample-exact ?seek_ms=.
        let passthrough_seek = seek.is_some() && fmt == StreamFormat::Passthrough;
        let opts = StreamOptions {
            format: Some(fmt),
            seek_ms: if passthrough_seek { None } else { seek },
            next: next_id,
            range_start: None,
        };
        let info = match self.transport.open_stream(track.id, &opts) {
            Ok(i) => i,
            Err(e) => {
                self.fail(&format!("stream failed: {e}"));
                return;
            }
        };
        let gapless_mode = info.gapless_mode.clone();
        let progress = info.progress.clone();
        let expect_chained = gapless_mode.as_deref() == Some("chained");
        let decoder = match StreamDecoder::new(info.reader, expect_chained) {
            Ok(d) => d,
            Err(e) => {
                self.fail(&format!("decode failed: {e}"));
                return;
            }
        };
        let spec = decoder.spec();

        // Segments: the server chained the next track's audio into this
        // response (single-session or chained mode).
        let mut segments = vec![track.clone()];
        let mut seg_frames = vec![expected_frames(track, spec.sample_rate)];
        if gapless_mode.is_some() {
            if let Some(next) = self.queue.peek_next().cloned() {
                // Sanity: the chained id must be the one we asked for.
                if Some(next.id) == info.gapless_next.or(next_id) {
                    seg_frames.push(expected_frames(&next, spec.sample_rate));
                    segments.push(next);
                }
            }
        }

        // Bit-perfect: open the exclusive device at the file's own rate when
        // the device can do exactly that; otherwise fall back to the shared
        // path (and say why) so playback never fails over this preference.
        let mut bit_perfect = false;
        if want_bp {
            self.sink.select_output_path(OutputPath::PcmExclusive);
            let rate_ok = self.sink.exclusive_pcm_rate(spec.sample_rate) == Some(spec.sample_rate);
            if !rate_ok {
                tracing::warn!(
                    track_id = track.id,
                    rate = spec.sample_rate,
                    "bit-perfect: output device cannot take this rate exclusively; using shared output"
                );
            } else if spec.channels == 0 || spec.channels > 8 {
                tracing::warn!(
                    track_id = track.id,
                    "bit-perfect: unsupported channel count; using shared output"
                );
            } else {
                match self
                    .sink
                    .open_exclusive_pcm(spec.sample_rate, spec.channels as u16)
                {
                    Ok(()) => bit_perfect = true,
                    Err(e) => tracing::warn!(
                        track_id = track.id,
                        error = %e,
                        "bit-perfect: could not open the exclusive device; using shared output"
                    ),
                }
            }
            if !bit_perfect {
                self.sink.select_output_path(OutputPath::Pcm);
            }
        }
        self.output_path = if bit_perfect {
            OutputPath::PcmExclusive
        } else {
            OutputPath::Pcm
        };

        let sink_rate;
        let resampler;
        if bit_perfect {
            // Untouched: native rate, no resampler.
            sink_rate = spec.sample_rate;
            resampler = None;
            if self.sink.play().is_err() {
                self.fail("audio sink failed to start");
                return;
            }
        } else {
            // Open the sink before the resample decision: C2's cpal sink
            // negotiates the device rate in open(), so the query below sees
            // the real rate (native when the device takes it).
            if self.sink.open(track).is_err() {
                self.fail("audio sink failed to open");
                return;
            }
            (resampler, sink_rate) = match self.sink.preferred_sample_rate() {
                Some(r) if r != spec.sample_rate => (
                    Some(CubicResampler::new(
                        spec.channels as usize,
                        spec.sample_rate,
                        r,
                    )),
                    r,
                ),
                _ => (None, spec.sample_rate),
            };
            if self.sink.play().is_err() {
                self.fail("audio sink failed to start");
                return;
            }
        }

        // The EQ runs on what the sink receives (after any resampling), so
        // it is designed at the sink rate, not the file's.
        self.eq.set_sample_rate(sink_rate);
        self.analog.prepare(sink_rate);

        // The playhead starts at the seek target for *both* seek styles.
        // (Passthrough skips decoded frames without counting them as
        // pumped, so leaving the base at 0 restarted the clock at 0:00
        // after every scrub.)
        let base_frames = seek.unwrap_or(0) * sink_rate as u64 / 1000;
        let skip_frames = if passthrough_seek {
            seek.unwrap_or(0) * spec.sample_rate as u64 / 1000
        } else {
            0
        };

        self.active = Some(ActiveStream::Pcm(PcmStream {
            segments,
            seg_idx: 0,
            seg_frames,
            gapless_mode,
            format_used: fmt,
            chain: info.chain,
            decoder,
            spec,
            resampler,
            sink_rate,
            decoded_frames: 0,
            pumped_frames: 0,
            base_frames,
            skip_frames,
            bit_perfect,
            progress,
        }));
        self.status = PlayerStatus::Playing;
    }

    /// Decode one chunk and push it to the sink. Call in a loop while
    /// [`wants_pump`](Self::wants_pump).
    pub fn pump(&mut self) {
        if self.status != PlayerStatus::Playing {
            return;
        }
        if self.active.is_none() {
            self.open_current(None);
            if self.active.is_none() {
                return;
            }
        }
        match &self.active {
            Some(ActiveStream::Pcm(_)) => self.pump_pcm(),
            Some(ActiveStream::Dop(_)) => self.pump_dop(),
            None => {}
        }
    }

    /// PCM pump: decode -> EQ -> loudness gain ramp -> volume -> sink.
    fn pump_pcm(&mut self) {
        // Decode one chunk.
        let channels = match self.active.as_ref() {
            Some(ActiveStream::Pcm(a)) => a.spec.channels as usize,
            _ => return,
        };
        let mut pcm = vec![0.0f32; CHUNK_FRAMES * channels];
        let decoded_frames = {
            let active = match self.active.as_mut() {
                Some(ActiveStream::Pcm(a)) => a,
                _ => return,
            };
            match active.decoder.decode_interleaved(&mut pcm) {
                Ok(n) => n,
                Err(e) => {
                    self.fail(&format!("decode failed: {e}"));
                    return;
                }
            }
        };
        if decoded_frames == 0 {
            // A response that never yields audio is skipped, but a streak
            // longer than the queue means every track is poison - fail
            // instead of looping forever (repeat-all would never end).
            let never_decoded =
                matches!(self.active.as_ref(), Some(ActiveStream::Pcm(a)) if a.decoded_frames == 0);
            if never_decoded {
                self.empty_streak += 1;
                if self.empty_streak > self.queue.len().max(1) as u32 {
                    self.fail("stream produced no audio");
                    return;
                }
            }
            self.on_stream_end();
            return;
        }
        self.empty_streak = 0;
        let active = match self.active.as_mut() {
            Some(ActiveStream::Pcm(a)) => a,
            _ => return,
        };
        active.decoded_frames += decoded_frames as u64;

        // Passthrough seek: drop frames until the target.
        let mut frames = &pcm[..decoded_frames * channels];
        if active.skip_frames > 0 {
            let drop = (active.skip_frames as usize).min(decoded_frames);
            frames = &frames[drop * channels..];
            active.skip_frames -= drop as u64;
        }
        if frames.is_empty() {
            return; // still skipping; no position advance
        }

        // Bit-perfect: straight to the exclusive device as packed 24-bit.
        // No EQ, loudness gain, volume or resampling touches the samples.
        if active.bit_perfect {
            let mut bytes = Vec::with_capacity(frames.len() * 3);
            f32_to_i24_le(frames, &mut bytes);
            let written = (frames.len() / channels) as u64;
            if self.sink.write_dop(&bytes).is_err() {
                self.fail("audio sink write failed");
                return;
            }
            if let Some(ActiveStream::Pcm(a)) = self.active.as_mut() {
                a.pumped_frames += written;
                a.advance_display();
            }
            return;
        }

        // Resample only when the sink demanded another rate.
        let resampled;
        let out: &[f32] = match active.resampler.as_mut() {
            Some(rs) => {
                resampled = rs.process(frames);
                &resampled
            }
            None => frames,
        };
        let out_frames = out.len() / channels;

        // v1 DSP chain, fixed order: EQ -> loudness gain (ramped) -> volume.
        let mut chunk: Vec<f32> = out.to_vec();
        self.eq.process(&mut chunk, channels);
        self.analog.process(&mut chunk, channels);
        self.gain_ramp.apply(&mut chunk);
        let vol = self.volume;
        if vol < 0.999 {
            for s in chunk.iter_mut() {
                *s *= vol;
            }
        }

        let sink_rate = active.sink_rate;
        let ch = active.spec.channels;
        if self
            .sink
            .write(PcmChunk {
                frames: chunk,
                sample_rate: sink_rate,
                channels: ch as u8,
            })
            .is_err()
        {
            self.fail("audio sink write failed");
            return;
        }
        let active = match self.active.as_mut() {
            Some(ActiveStream::Pcm(a)) => a,
            _ => return,
        };
        active.pumped_frames += out_frames as u64;
        active.advance_display();
    }

    /// DoP pump: raw DoP frames straight to the exclusive sink. No decode,
    /// no EQ, no loudness, no volume, no resample - bit-perfect by
    /// construction (see `dop_bypasses_dsp` in engine_tests).
    fn pump_dop(&mut self) {
        let frame_bytes = match self.active.as_ref() {
            Some(ActiveStream::Dop(a)) => a.spec.frame_bytes(),
            _ => return,
        };
        // Whole frames only: the sink must never see a partial DoP frame.
        let mut buf = vec![0u8; 4096 * frame_bytes];
        let n = {
            let active = match self.active.as_mut() {
                Some(ActiveStream::Dop(a)) => a,
                _ => return,
            };
            match active.stream.read_frames(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    self.fail(&format!("DoP read failed: {e}"));
                    return;
                }
            }
        };
        if n == 0 {
            let never_produced =
                matches!(self.active.as_ref(), Some(ActiveStream::Dop(a)) if a.pumped_frames == 0);
            if never_produced {
                self.empty_streak += 1;
                if self.empty_streak > self.queue.len().max(1) as u32 {
                    self.fail("DoP stream produced no audio");
                    return;
                }
            }
            self.on_stream_end();
            return;
        }
        self.empty_streak = 0;
        if self.sink.write_dop(&buf[..n]).is_err() {
            self.fail("DoP sink write failed");
            return;
        }
        if let Some(ActiveStream::Dop(a)) = self.active.as_mut() {
            a.pumped_frames += (n / frame_bytes) as u64;
            a.advance_display();
        }
    }

    /// The response is fully consumed: advance past the tracks whose audio
    /// actually played and open the next one.
    fn on_stream_end(&mut self) {
        // Let the tail of the finished stream play out; opening the next
        // track (or stopping) discards whatever is still buffered.
        self.sink.drain();
        let consumed = self
            .active
            .as_ref()
            .map(|a| a.tracks_consumed())
            .unwrap_or(1);
        self.active = None;
        for _ in 0..consumed {
            if self.queue.next_track().is_none() {
                break;
            }
        }
        self.persist_queue();
        if self.queue.current().is_some() {
            self.open_current(None);
        } else {
            self.stop();
        }
    }

    fn fail(&mut self, msg: &str) {
        self.error = Some(msg.to_string());
        self.status = PlayerStatus::Stopped;
        self.active = None;
        let _ = self.sink.stop();
    }

    // -- introspection -----------------------------------------------------

    /// Audible playhead of the active stream (0 when idle).
    fn position_ms(&self) -> u64 {
        let buffered = self.sink.buffered_frames();
        self.active
            .as_ref()
            .map(|a| a.position_ms(buffered))
            .unwrap_or(0)
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        let (track, position_ms, duration_ms, buffered_ms, output_rate_hz, format, chain) = match &self.active {
            Some(a) => {
                let t = a.display_track().clone();
                let pos = self.position_ms();
                (
                    Some(t.clone()),
                    pos,
                    t.duration_ms,
                    a.buffered_ms(pos),
                    Some(a.output_rate_hz()),
                    Some(a.format_used()),
                    a.chain().clone(),
                )
            }
            None => {
                // Idle. A restored queue shows where it will resume.
                let t = self.queue.current().cloned();
                let at = self.resume_at_ms.unwrap_or(0);
                let dur = t.as_ref().and_then(|t| t.duration_ms).filter(|_| at > 0);
                (t, at, dur, None, None, None, None)
            }
        };
        PlayerSnapshot {
            status: self.status,
            track,
            queue_ids: self.queue.ordered_ids(),
            queue_index: self.queue.current().map(|_| self.queue.index()),
            current_id: self.queue.current().map(|t| t.id),
            position_ms,
            duration_ms,
            buffered_ms,
            output_rate_hz,
            analog_plan: if self.active.is_some() && self.output_path == OutputPath::Pcm {
                self.analog.status().map(|s| s.describe())
            } else {
                None
            },
            format,
            chain,
            output_path: self.output_path,
            volume: self.volume,
            error: self.error.clone(),
            repeat: self.queue.repeat,
            shuffle: self.queue.shuffle,
        }
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn status(&self) -> PlayerStatus {
        self.status
    }
}

/// The `idx`-th part of an `X-Transcode-Chain` value that lists one chain per
/// chained track, separated by " + ". A value with fewer parts (or none)
/// applies to every segment, so it falls back to the whole string.
fn chain_segment(chain: &str, idx: usize) -> &str {
    if !chain.contains(" + ") {
        return chain;
    }
    chain.split(" + ").nth(idx).map(str::trim).unwrap_or(chain)
}

/// Expected frames of a track at `rate`, from catalog metadata. Only used
/// to move the *displayed* track across an invisible server-side gapless
/// boundary; the audio path never depends on it.
fn expected_frames(track: &Track, rate: u32) -> u64 {
    track.duration_ms.unwrap_or(0) * rate as u64 / 1000
}

// ---------------------------------------------------------------------------
// EngineController: owns the playback thread
// ---------------------------------------------------------------------------

/// Commands the Tauri shell (or tests) send to the playback thread.
#[derive(Debug)]
pub enum EngineCommand {
    PlayQueue(Vec<Track>, usize),
    Pause,
    Resume,
    Toggle,
    Stop,
    Seek(u64),
    Next,
    Prev,
    SetGlobalFormat(Option<StreamFormat>),
    SetTrackFormat(i64, Option<StreamFormat>),
    /// DSD handling preference (Settings); see [`DsdStory`].
    SetDsdStory(DsdStory),
    /// Repeat-off/all/one. The queue owns the mode; the engine just
    /// forwards it and persists.
    SetRepeat(RepeatMode),
    SetShuffle(bool),
    /// Append to the end of the queue without disturbing playback
    /// ("add to queue").
    AppendTracks(Vec<Track>),
    /// Insert right after the current item in playback order ("play next").
    InsertTracksNext(Vec<Track>),
    /// Restore a persisted queue without starting playback (launch).
    RestoreQueue {
        tracks: Vec<Track>,
        index: usize,
        repeat: RepeatMode,
        shuffle: bool,
        position_ms: u64,
    },
    /// PCM only; has no effect on DoP (bit-perfect hog mode).
    SetVolume(f32),
    /// Output device by name; `None` = system default.
    SetOutputDevice(Option<String>),
    /// When to use exclusive bit-perfect PCM output.
    SetBitPerfect(BitPerfect),
    /// Replace the parametric EQ bands (validated; rejected wholesale if
    /// any band is invalid). PCM only.
    SetEqBands(Vec<EqBand>),
    /// PCM only.
    SetEqEnabled(bool),
    SetAnalog(AnalogSettings),
    /// Target integrated loudness in LUFS (e.g. -14.0). PCM only.
    SetLoudnessTarget(f32),
    /// PCM only; enables the pre-scan (one extra stream per first-play).
    SetLoudnessEnabled(bool),
    SetServerUrl(String),
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EngineSettings {
    server_url: String,
    /// DSP settings the shell mirrors; `#[serde(default)]` keeps old
    /// settings files (server URL only) loadable.
    #[serde(default)]
    dsp: DspSettings,
    /// Client DSD preference (Settings → DSD handling). Defaults to
    /// `Convert`, preserving pre-C3 behavior.
    #[serde(default)]
    dsd_story: DsdStory,
    /// Global format override (`None` = auto/ladder). Persisted so the
    /// S13 preference survives restarts.
    #[serde(default)]
    global_format: Option<StreamFormat>,
    /// Chosen output device name (`None` = system default). Names are the
    /// only identity cpal/CoreAudio give us; a device that is unplugged
    /// falls back to the default at open time.
    #[serde(default)]
    output_device: Option<String>,
    /// Exclusive bit-perfect output preference (default off).
    #[serde(default)]
    bit_perfect: BitPerfect,
}

/// Queue state persisted to `queue.json` next to the engine settings file
/// (design: "Queue … persisted to disk, restored on launch"). Written by
/// the playback thread on every queue mutation; restored once at startup
/// with status Stopped (the UI hydrates from the snapshot and the user
/// presses play). A separate file from the settings avoids read-modify-
/// write races between the controller thread (DSP/URL prefs) and the
/// playback thread (queue).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedQueue {
    tracks: Vec<Track>,
    index: usize,
    repeat: RepeatMode,
    shuffle: bool,
    /// Playhead inside the current track when it was last saved; playback
    /// resumes from here after a restart.
    #[serde(default)]
    position_ms: u64,
}

impl PersistedQueue {
    fn load(path: &PathBuf) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }
}

/// `queue.json` lives next to `engine-settings.json`.
fn queue_path_for(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .map(|d| d.join("queue.json"))
        .unwrap_or_else(|| PathBuf::from("queue.json"))
}

impl EngineSettings {
    fn load(path: &PathBuf) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Self {
                server_url: DEFAULT_SERVER_URL.to_string(),
                dsp: DspSettings::default(),
                dsd_story: DsdStory::default(),
                global_format: None,
                output_device: None,
                bit_perfect: BitPerfect::default(),
            })
    }

    fn save(&self, path: &PathBuf) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, t);
        }
    }
}

/// Owns the playback thread and the shared snapshot. All mutating calls
/// are fire-and-forget; state is observed via [`snapshot`](Self::snapshot)
/// and [`drain_events`](Self::drain_events).
pub struct EngineController {
    tx: mpsc::Sender<EngineCommand>,
    snapshot: Arc<Mutex<PlayerSnapshot>>,
    events: Arc<Mutex<Vec<PlayerEvent>>>,
    server_url: Arc<RwLock<String>>,
    dsd_story: Arc<RwLock<DsdStory>>,
    output_device: Arc<RwLock<Option<String>>>,
    bit_perfect: Arc<RwLock<BitPerfect>>,
    global_format: Arc<RwLock<Option<StreamFormat>>>,
    settings_path: PathBuf,
    _thread: JoinHandle<()>,
}

impl EngineController {
    /// `settings_path`: JSON file holding the server URL and DSP settings
    /// (the Tauri shell passes its app-data dir; tests pass a temp path).
    /// The persisted queue (`queue.json` next to it) is restored with
    /// status Stopped — the UI hydrates from the snapshot on launch.
    pub fn new(sink: Box<dyn AudioSink>, settings_path: PathBuf) -> Self {
        let settings = EngineSettings::load(&settings_path);
        let server_url = Arc::new(RwLock::new(settings.server_url.clone()));
        let transport: Box<dyn Transport> =
            Box::new(HttpTransport::with_shared_url(server_url.clone()));
        let ctrl = Self::with_transport(sink, transport, server_url, settings_path);
        // Apply persisted DSP settings before any playback.
        let dsp = &settings.dsp;
        ctrl.send(EngineCommand::SetEqBands(dsp.eq_bands.clone()));
        ctrl.send(EngineCommand::SetEqEnabled(dsp.eq_enabled));
        ctrl.send(EngineCommand::SetAnalog(dsp.analog));
        ctrl.send(EngineCommand::SetLoudnessTarget(dsp.loudness_target));
        ctrl.send(EngineCommand::SetLoudnessEnabled(dsp.loudness_enabled));
        // Playback preferences (persisted; default preserves pre-C3 behavior).
        ctrl.send(EngineCommand::SetDsdStory(settings.dsd_story));
        ctrl.send(EngineCommand::SetGlobalFormat(settings.global_format));
        *ctrl.dsd_story.write().expect("dsd lock") = settings.dsd_story;
        // Output device (before any playback opens the sink).
        ctrl.send(EngineCommand::SetOutputDevice(
            settings.output_device.clone(),
        ));
        *ctrl.output_device.write().expect("device lock") = settings.output_device.clone();
        ctrl.send(EngineCommand::SetBitPerfect(settings.bit_perfect));
        *ctrl.bit_perfect.write().expect("bit-perfect lock") = settings.bit_perfect;
        *ctrl.global_format.write().expect("format lock") = settings.global_format;
        // Restore the persisted queue, if any. Repeat/shuffle ride along in
        // the queue file so the restore is exact (shuffle order is
        // deterministic, so the persisted cursor still points at the same
        // track).
        let qp = queue_path_for(&ctrl.settings_path);
        if let Some(pq) = PersistedQueue::load(&qp) {
            if !pq.tracks.is_empty() {
                ctrl.send(EngineCommand::RestoreQueue {
                    tracks: pq.tracks,
                    index: pq.index,
                    repeat: pq.repeat,
                    shuffle: pq.shuffle,
                    position_ms: pq.position_ms,
                });
            }
        }
        ctrl
    }

    /// The tracks in the saved queue (`queue.json`), in list order. Lets the
    /// UI show a restored queue without asking the server for every track.
    pub fn saved_queue_tracks(&self) -> Vec<Track> {
        PersistedQueue::load(&queue_path_for(&self.settings_path))
            .map(|pq| pq.tracks)
            .unwrap_or_default()
    }

    /// Test seam: inject any [`Transport`] (the live constructor always
    /// uses [`HttpTransport`]).
    pub fn with_transport(
        sink: Box<dyn AudioSink>,
        transport: Box<dyn Transport>,
        server_url: Arc<RwLock<String>>,
        settings_path: PathBuf,
    ) -> Self {
        let mut player = Player::new(sink, transport);
        player.set_queue_path(Some(queue_path_for(&settings_path)));
        let (tx, rx) = mpsc::channel::<EngineCommand>();
        let snapshot = Arc::new(Mutex::new(PlayerSnapshot::default()));
        let events = Arc::new(Mutex::new(Vec::new()));

        let snap2 = snapshot.clone();
        let ev2 = events.clone();
        let thread = std::thread::Builder::new()
            .name("player-playback".into())
            .spawn(move || playback_loop(rx, player, snap2, ev2))
            .expect("spawn playback thread");

        Self {
            tx,
            snapshot,
            events,
            server_url,
            dsd_story: Arc::new(RwLock::new(DsdStory::default())),
            output_device: Arc::new(RwLock::new(None)),
            bit_perfect: Arc::new(RwLock::new(BitPerfect::default())),
            global_format: Arc::new(RwLock::new(None)),
            settings_path,
            _thread: thread,
        }
    }

    fn send(&self, cmd: EngineCommand) {
        let _ = self.tx.send(cmd);
    }

    pub fn play_queue(&self, tracks: Vec<Track>, index: usize) {
        self.send(EngineCommand::PlayQueue(tracks, index));
    }
    pub fn pause(&self) {
        self.send(EngineCommand::Pause);
    }
    pub fn resume(&self) {
        self.send(EngineCommand::Resume);
    }
    pub fn toggle(&self) {
        self.send(EngineCommand::Toggle);
    }
    pub fn stop(&self) {
        self.send(EngineCommand::Stop);
    }
    pub fn seek_ms(&self, ms: u64) {
        self.send(EngineCommand::Seek(ms));
    }
    pub fn next(&self) {
        self.send(EngineCommand::Next);
    }
    pub fn prev(&self) {
        self.send(EngineCommand::Prev);
    }
    pub fn set_global_format(&self, fmt: Option<StreamFormat>) {
        *self.global_format.write().expect("format lock") = fmt;
        let mut settings = EngineSettings::load(&self.settings_path);
        settings.global_format = fmt;
        settings.save(&self.settings_path);
        self.send(EngineCommand::SetGlobalFormat(fmt));
    }

    /// DSD handling preference (Settings). Persisted; consulted for DSD
    /// tracks when no explicit format override applies.
    pub fn set_dsd_story(&self, story: DsdStory) {
        *self.dsd_story.write().expect("dsd lock") = story;
        let mut settings = EngineSettings::load(&self.settings_path);
        settings.dsd_story = story;
        settings.save(&self.settings_path);
        self.send(EngineCommand::SetDsdStory(story));
    }

    /// Choose the output device by name (`None` = system default).
    /// Persisted; applies immediately (a playing track moves over at its
    /// current position).
    pub fn set_output_device(&self, name: Option<String>) {
        *self.output_device.write().expect("device lock") = name.clone();
        let mut settings = EngineSettings::load(&self.settings_path);
        settings.output_device = name.clone();
        settings.save(&self.settings_path);
        self.send(EngineCommand::SetOutputDevice(name));
    }

    /// Choose when to use exclusive bit-perfect output. Persisted; applies
    /// immediately to a track that is playing.
    pub fn set_bit_perfect(&self, mode: BitPerfect) {
        *self.bit_perfect.write().expect("bit-perfect lock") = mode;
        let mut settings = EngineSettings::load(&self.settings_path);
        settings.bit_perfect = mode;
        settings.save(&self.settings_path);
        self.send(EngineCommand::SetBitPerfect(mode));
    }

    pub fn bit_perfect(&self) -> BitPerfect {
        *self.bit_perfect.read().expect("bit-perfect lock")
    }

    /// The chosen output device (`None` = system default).
    pub fn output_device(&self) -> Option<String> {
        self.output_device.read().expect("device lock").clone()
    }

    pub fn set_repeat(&self, mode: RepeatMode) {
        self.send(EngineCommand::SetRepeat(mode));
    }

    pub fn set_shuffle(&self, on: bool) {
        self.send(EngineCommand::SetShuffle(on));
    }

    /// Append tracks to the end of the queue without disturbing playback.
    pub fn append_tracks(&self, tracks: Vec<Track>) {
        self.send(EngineCommand::AppendTracks(tracks));
    }

    /// Insert tracks right after the current item ("play next").
    pub fn insert_tracks_next(&self, tracks: Vec<Track>) {
        self.send(EngineCommand::InsertTracksNext(tracks));
    }

    /// Authoritative playback preferences for the Settings UI
    /// (repeat/shuffle ride in the player-state snapshot instead).
    pub fn playback_prefs(&self) -> (DsdStory, Option<StreamFormat>) {
        (
            *self.dsd_story.read().expect("dsd lock"),
            *self.global_format.read().expect("format lock"),
        )
    }
    pub fn set_track_format(&self, track_id: i64, fmt: Option<StreamFormat>) {
        self.send(EngineCommand::SetTrackFormat(track_id, fmt));
    }
    pub fn set_volume(&self, v: f32) {
        self.send(EngineCommand::SetVolume(v));
    }

    /// Replace the parametric EQ bands (validated engine-side; invalid
    /// bands are rejected with a warning). PCM only. Persisted.
    pub fn set_eq_bands(&self, bands: Vec<EqBand>) {
        self.update_dsp_settings(|dsp| dsp.eq_bands = bands.clone());
        self.send(EngineCommand::SetEqBands(bands));
    }

    /// Persisted.
    pub fn set_eq_enabled(&self, enabled: bool) {
        self.update_dsp_settings(|dsp| dsp.eq_enabled = enabled);
        self.send(EngineCommand::SetEqEnabled(enabled));
    }
    /// Analog character (tube / transistor warmth). Values are clamped to
    /// their ranges; the result is saved and applied live.
    pub fn set_analog(&self, settings: AnalogSettings) {
        let settings = settings.clamped();
        self.update_dsp_settings(|dsp| dsp.analog = settings);
        self.send(EngineCommand::SetAnalog(settings));
    }

    /// Persisted.
    pub fn set_loudness_target(&self, lufs: f32) {
        self.update_dsp_settings(|dsp| dsp.loudness_target = lufs);
        self.send(EngineCommand::SetLoudnessTarget(lufs));
    }

    /// Enabling triggers one extra stream per first-play (pre-scan);
    /// gains are then cached by (track, format). Persisted.
    pub fn set_loudness_enabled(&self, enabled: bool) {
        self.update_dsp_settings(|dsp| dsp.loudness_enabled = enabled);
        self.send(EngineCommand::SetLoudnessEnabled(enabled));
    }

    /// Read-modify-write the DSP section of the settings file. Each setter
    /// passes the complete new value, so this never needs the engine's
    /// current state.
    fn update_dsp_settings(&self, f: impl FnOnce(&mut DspSettings)) {
        let mut settings = EngineSettings::load(&self.settings_path);
        f(&mut settings.dsp);
        settings.save(&self.settings_path);
    }

    pub fn server_url(&self) -> String {
        self.server_url.read().expect("url lock").clone()
    }

    pub fn set_server_url(&self, url: &str) {
        *self.server_url.write().expect("url lock") = url.to_string();
        let mut settings = EngineSettings::load(&self.settings_path);
        settings.server_url = url.to_string();
        settings.save(&self.settings_path);
        self.send(EngineCommand::SetServerUrl(url.to_string()));
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        self.snapshot.lock().expect("snapshot lock").clone()
    }

    /// Events since the last call, for the shell to forward to the UI.
    pub fn drain_events(&self) -> Vec<PlayerEvent> {
        std::mem::take(&mut *self.events.lock().expect("events lock"))
    }
}

impl Drop for EngineController {
    fn drop(&mut self) {
        let _ = self.tx.send(EngineCommand::Shutdown);
    }
}

fn playback_loop(
    rx: mpsc::Receiver<EngineCommand>,
    mut player: Player,
    snapshot: Arc<Mutex<PlayerSnapshot>>,
    events: Arc<Mutex<Vec<PlayerEvent>>>,
) {
    let mut last_emit = Instant::now() - Duration::from_secs(10);
    let mut last_saved = Instant::now();
    let mut last_key = snapshot_key(&PlayerSnapshot::default());

    loop {
        // Drain pending commands.
        let mut shutdown = false;
        while let Ok(cmd) = rx.try_recv() {
            if matches!(cmd, EngineCommand::Shutdown) {
                shutdown = true;
                break;
            }
            apply_command(&mut player, cmd);
        }
        if shutdown {
            break;
        }

        if player.wants_pump() {
            player.pump();
        } else {
            // Idle: block briefly so the thread sleeps instead of spinning.
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(EngineCommand::Shutdown) => break,
                Ok(cmd) => apply_command(&mut player, cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Emit ~4 Hz while playing, immediately on track/state changes.
        // The key covers everything the UI renders from the snapshot
        // (queue contents, repeat/shuffle modes, …) so e.g. "add to queue"
        // from another view refreshes the queue UI even while idle —
        // position_ms is deliberately excluded and stays on the 4 Hz
        // throttle while playing.
        let snap = player.snapshot();
        let key = snapshot_key(&snap);
        let changed = key != last_key;
        if player.status() == PlayerStatus::Playing && last_saved.elapsed() >= Duration::from_secs(5) {
            last_saved = Instant::now();
            player.persist_position();
        }
        if changed || (player.wants_pump() && last_emit.elapsed() >= Duration::from_millis(250)) {
            last_key = key;
            last_emit = Instant::now();
            *snapshot.lock().expect("snapshot lock") = snap.clone();
            events
                .lock()
                .expect("events lock")
                .push(PlayerEvent::State(snap));
        }
    }
}

/// UI-visible snapshot identity minus the ever-moving playhead.
fn snapshot_key(
    s: &PlayerSnapshot,
) -> (
    PlayerStatus,
    Option<i64>,
    Vec<i64>,
    RepeatMode,
    bool,
    u32,
    u64,
    OutputPath,
) {
    // Volume changes must reach the UI even while paused/stopped, and a
    // seek while paused moves the (otherwise static) playhead. While
    // playing the position rides the 4 Hz throttle instead.
    let idle_position = if s.status == PlayerStatus::Playing {
        0
    } else {
        s.position_ms
    };
    (
        s.status,
        s.current_id,
        s.queue_ids.clone(),
        s.repeat,
        s.shuffle,
        s.volume.to_bits(),
        idle_position,
        // The badge/volume behaviour depends on it, and switching the
        // bit-perfect mode while paused must still reach the UI.
        s.output_path,
    )
}

fn apply_command(player: &mut Player, cmd: EngineCommand) {
    match cmd {
        EngineCommand::PlayQueue(tracks, index) => player.play_queue(tracks, index),
        EngineCommand::Pause => player.pause(),
        EngineCommand::Resume => player.resume(),
        EngineCommand::Toggle => player.toggle(),
        EngineCommand::Stop => player.stop(),
        EngineCommand::Seek(ms) => player.seek_ms(ms),
        EngineCommand::Next => player.next(),
        EngineCommand::Prev => player.prev(),
        EngineCommand::SetGlobalFormat(f) => player.set_global_format(f),
        EngineCommand::SetTrackFormat(id, f) => player.set_track_format(id, f),
        EngineCommand::SetDsdStory(s) => player.set_dsd_story(s),
        EngineCommand::SetRepeat(m) => player.set_repeat(m),
        EngineCommand::SetShuffle(b) => player.set_shuffle(b),
        EngineCommand::AppendTracks(t) => player.append_tracks(t),
        EngineCommand::InsertTracksNext(t) => player.insert_tracks_next(t),
        EngineCommand::RestoreQueue {
            tracks,
            index,
            repeat,
            shuffle,
            position_ms,
        } => player.restore_queue(tracks, index, repeat, shuffle, position_ms),
        EngineCommand::SetVolume(v) => player.set_volume(v),
        EngineCommand::SetOutputDevice(d) => player.set_output_device(d),
        EngineCommand::SetBitPerfect(m) => player.set_bit_perfect(m),
        EngineCommand::SetEqBands(bands) => {
            if let Err(e) = player.set_eq_bands(bands) {
                tracing::warn!("rejected EQ bands: {e}");
            }
        }
        EngineCommand::SetEqEnabled(b) => player.set_eq_enabled(b),
        EngineCommand::SetAnalog(a) => player.set_analog(a),
        EngineCommand::SetLoudnessTarget(t) => player.set_loudness_target(t),
        EngineCommand::SetLoudnessEnabled(b) => player.set_loudness_enabled(b),
        EngineCommand::SetServerUrl(_) => {
            // The transport reads the shared URL lock directly; nothing to do.
        }
        EngineCommand::Shutdown => {}
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    fn snap() -> PlayerSnapshot {
        PlayerSnapshot::default()
    }

    #[test]
    fn volume_change_changes_the_key_in_every_state() {
        for status in [
            PlayerStatus::Playing,
            PlayerStatus::Paused,
            PlayerStatus::Stopped,
        ] {
            let a = PlayerSnapshot { status, ..snap() };
            let b = PlayerSnapshot {
                status,
                volume: 0.3,
                ..snap()
            };
            assert_ne!(snapshot_key(&a), snapshot_key(&b), "{status:?}");
        }
    }

    #[test]
    fn output_path_change_changes_the_key() {
        let a = PlayerSnapshot {
            output_path: OutputPath::Pcm,
            ..snap()
        };
        let b = PlayerSnapshot {
            output_path: OutputPath::PcmExclusive,
            ..snap()
        };
        assert_ne!(snapshot_key(&a), snapshot_key(&b));
    }

    #[test]
    fn playhead_moves_the_key_only_when_not_playing() {
        let moved = |status| {
            let a = PlayerSnapshot {
                status,
                position_ms: 100,
                ..snap()
            };
            let b = PlayerSnapshot {
                status,
                position_ms: 900,
                ..snap()
            };
            snapshot_key(&a) != snapshot_key(&b)
        };
        assert!(
            !moved(PlayerStatus::Playing),
            "playing rides the 4 Hz throttle"
        );
        assert!(
            moved(PlayerStatus::Paused),
            "a paused seek must reach the UI"
        );
        assert!(moved(PlayerStatus::Stopped));
    }
}

#[cfg(test)]
mod chain_tests {
    use super::chain_segment;

    #[test]
    fn a_single_chain_is_returned_unchanged() {
        assert_eq!(chain_segment("wav->passthrough", 0), "wav->passthrough");
        assert_eq!(
            chain_segment("wav->passthrough", 1),
            "wav->passthrough",
            "later segments share it"
        );
        assert_eq!(chain_segment("", 0), "");
    }

    #[test]
    fn a_gapless_response_shows_the_chain_of_the_track_being_heard() {
        let both = "dsf64->flac 24/88.2 + dsf128->flac 24/176.4";
        assert_eq!(chain_segment(both, 0), "dsf64->flac 24/88.2");
        assert_eq!(chain_segment(both, 1), "dsf128->flac 24/176.4");
    }

    #[test]
    fn an_out_of_range_segment_falls_back_to_the_whole_string() {
        let both = "a->b + c->d";
        assert_eq!(chain_segment(both, 5), both);
    }

    #[test]
    fn identical_chains_collapse_to_one() {
        // The case from the bug report: two DSF64 tracks back to back.
        let both = "dsf64->flac 24/88.2 + dsf64->flac 24/88.2";
        assert_eq!(chain_segment(both, 0), "dsf64->flac 24/88.2");
        assert!(!chain_segment(both, 0).contains('+'));
    }
}
