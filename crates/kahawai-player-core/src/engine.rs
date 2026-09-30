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

use crate::analog::{AnalogSettings, AnalogStage};
use crate::bitperfect::{f32_to_i24_le, BitPerfect};
use crate::crossfeed::{CrossfeedSettings, CrossfeedStage};
use crate::decode::{DecodedSpec, StreamDecoder};
use crate::dop::{dop_pcm_rate, DopSpec, DopStream};
use crate::dsp::{
    headroom_guard, scan_track_levels, DspStage, EqBand, GainRamp, LookaheadLimiter, LoudnessMeter,
    LoudnessNorm, ParametricEq, DEFAULT_LOUDNESS_TARGET,
};
use crate::quality::{
    QualityMode, BLOCKER_ANALOG, BLOCKER_CROSSFEED, BLOCKER_EQ, BLOCKER_LIMITER, BLOCKER_LOUDNESS,
    BLOCKER_VOLUME,
};
use crate::queue::{Queue, RepeatMode};
use crate::resample::CubicResampler;
use crate::sink::{AudioSink, OutputPath, PcmChunk};
use crate::transport::{HttpTransport, StreamOptions, StreamProgress, Transport};

/// Frames decoded per pump iteration (~93 ms at 44.1 kHz).
const CHUNK_FRAMES: usize = 4096;
/// How fast the limiter's gain-reduction meter falls back once the reduction
/// eases. Fast enough to track the music, slow enough to be readable.
const GR_DECAY_DB_PER_SEC: f32 = 20.0;

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
/// DoP rate); `Convert` transcodes to PCM/FLAC; `Auto` (default) is `Native`
/// for output devices known to decode DoP (see [`crate::dsd_devices`]) and
/// `Convert` for everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DsdStory {
    Native,
    Convert,
    #[default]
    Auto,
}

/// Resolve the rendition for a track: per-track override wins; then, for DSD
/// tracks under the `Native` story, DoP (a *global* format does not apply to
/// DSD there — otherwise a stray "Passthrough" would silently defeat the DSD
/// setting); then the global preference; then the desktop ladder
/// (passthrough when the source is directly streamable, else FLAC). Mirrors
/// the server's default ladder; an explicit per-track choice is always
/// honored as-is (including `dop`). `Auto` must be resolved to `Native` or
/// `Convert` by the caller (the engine does, from the output device); here
/// it is treated as `Convert`.
pub fn resolve_format(
    track: &Track,
    global: Option<StreamFormat>,
    per_track: Option<StreamFormat>,
    dsd_story: DsdStory,
) -> StreamFormat {
    if let Some(f) = per_track {
        return f;
    }
    let is_dsd = matches!(track.format, AudioFormat::Dsf | AudioFormat::Dff);
    if dsd_story == DsdStory::Native && is_dsd {
        return StreamFormat::Dop;
    }
    if let Some(f) = global {
        return f;
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

/// The analog stage's effect on the level (K-weighted, smoothed over a few
/// seconds). Relative readings, for level-matching an A/B comparison.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AnalogLevel {
    /// Loudness going into the stage (LUFS-like).
    pub input_lufs: f32,
    /// Loudness coming out of it.
    pub output_lufs: f32,
    /// Output minus input, in dB: what the stage adds to the level.
    pub delta_db: f32,
    /// Output peak, dBFS, decaying about 6 dB per second.
    pub peak_dbfs: f32,
    /// Seconds of audio behind the reading.
    pub seconds: f32,
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
    /// How the analog stage changes the level: loudness before and after, and
    /// the output peak. `None` when the stage is off or nothing has been measured yet.
    #[serde(default)]
    pub analog_level: Option<AnalogLevel>,
    /// Look-ahead limiter gain reduction in dB (positive, 0 = not working).
    /// `None` when the limiter is off or the path bypasses it.
    #[serde(default)]
    pub limiter_gr_db: Option<f32>,
    /// The rendition actually streaming (explicit `?format=` value).
    pub format: Option<StreamFormat>,
    /// `X-Transcode-Chain` of the active response.
    pub chain: Option<String>,
    /// Active output path: `pcm-shared` (DSP chain live) or
    /// `dop-exclusive` (hog-mode DoP, DSP bypassed).
    pub output_path: OutputPath,
    pub volume: f32,
    pub error: Option<String>,
    /// A non-fatal explanation for this track, e.g. why DSD played as FLAC
    /// instead of native DoP. Cleared on the next track.
    #[serde(default)]
    pub notice: Option<String>,
    /// The user's own processing (EQ, Loudness, Analog, Volume) that
    /// exclusive output would bypass right now.
    #[serde(default)]
    pub exclusive_blockers: Vec<String>,
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
            analog_level: None,
            limiter_gr_db: None,
            format: None,
            chain: None,
            output_path: OutputPath::Pcm,
            volume: 1.0,
            error: None,
            notice: None,
            exclusive_blockers: Vec::new(),
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
    /// Look-ahead limiter on the shared path. Off by default, like loudness
    /// and analog, so Auto bit-perfect / native DSD still works out of the
    /// box on a fresh install.
    #[serde(default)]
    pub limiter_enabled: bool,
    /// Headphone crossfeed. Older settings files have none: it defaults to
    /// off, like the other stages.
    #[serde(default)]
    pub crossfeed: CrossfeedSettings,
}

impl Default for DspSettings {
    fn default() -> Self {
        Self {
            eq_bands: Vec::new(),
            eq_enabled: true,
            loudness_enabled: false,
            loudness_target: DEFAULT_LOUDNESS_TARGET,
            analog: AnalogSettings::default(),
            limiter_enabled: false,
            crossfeed: CrossfeedSettings::default(),
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

    /// The track the server already chained into this response after the
    /// one being heard (gapless), if any.
    fn chained_next_id(&self) -> Option<i64> {
        match self {
            ActiveStream::Pcm(a) => a.segments.get(a.seg_idx + 1).map(|t| t.id),
            ActiveStream::Dop(a) => a.segments.get(a.seg_idx + 1).map(|t| t.id),
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
    /// Output devices the user confirmed as DoP-capable (for `Auto`).
    dsd_devices: Vec<String>,
    /// The top-level quality mode; Auto settings follow it.
    quality: QualityMode,
    /// The output device the user chose (`None` = system default), to tell
    /// when the sink had to fall back because it isn't connected.
    chosen_device: Option<String>,
    /// A notice to show once the next open starts (set by recovery paths,
    /// because opening a track clears the notice).
    pending_notice: Option<String>,
    /// After an exclusive output was lost mid-track, this track re-opens on
    /// shared output instead of trying exclusive again.
    skip_exclusive_track: Option<i64>,
    /// What `auto_wants_exclusive` said when the loaded track was opened, so a
    /// later change to the user's processing that flips the answer can re-open it.
    exclusive_decision: bool,
    /// When to use the exclusive, untouched PCM path (see `bitperfect`).
    bit_perfect: BitPerfect,
    track_formats: HashMap<i64, StreamFormat>,
    volume: f32,
    active: Option<ActiveStream>,
    error: Option<String>,
    notice: Option<String>,
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
    crossfeed: CrossfeedStage,
    eq: ParametricEq,
    analog: AnalogStage,
    /// Level before and after the analog stage, to compare them.
    meter_in: LoudnessMeter,
    meter_out: LoudnessMeter,
    /// Output peak (linear), decaying about 6 dB per second.
    peak_out: f32,
    loudness: LoudnessNorm,
    /// Click-free gain transitions between tracks with different
    /// loudness gains (~50 ms linear ramp).
    gain_ramp: GainRamp,
    /// Final safety stage: catches an EQ/gain overshoot before it reaches
    /// full scale, transparently, instead of `headroom_guard` bending into
    /// it reactively. Flushed (not just reset) when a track's decoder is
    /// genuinely exhausted — see `pump_pcm`.
    limiter: LookaheadLimiter,
    /// Off puts the shared path back on `headroom_guard` alone (a reactive
    /// soft clip). DSP like EQ/loudness/analog, so it blocks exclusive
    /// output the same way (`exclusive_blockers`). Off by default, like
    /// loudness and analog, so Auto bit-perfect / native DSD still works
    /// out of the box.
    limiter_enabled: bool,
    /// Gain reduction for the meter (dB, positive), peak-held then decayed
    /// like `peak_out` so a brief duck stays visible between snapshots
    /// (which the UI only gets about four times a second).
    limiter_gr_db: f32,
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
            dsd_devices: Vec::new(),
            quality: QualityMode::default(),
            chosen_device: None,
            pending_notice: None,
            skip_exclusive_track: None,
            exclusive_decision: false,
            bit_perfect: BitPerfect::default(),
            track_formats: HashMap::new(),
            volume: 1.0,
            active: None,
            error: None,
            notice: None,
            queue_path: None,
            resume_at_ms: None,
            empty_streak: 0,
            crossfeed: CrossfeedStage::new(44100),
            eq: ParametricEq::new(44100),
            analog: AnalogStage::new(44100),
            meter_in: LoudnessMeter::new(44100),
            meter_out: LoudnessMeter::new(44100),
            peak_out: 0.0,
            loudness: LoudnessNorm::new(DEFAULT_LOUDNESS_TARGET),
            gain_ramp: GainRamp::new(2205),
            limiter: LookaheadLimiter::new(44100),
            limiter_enabled: false,
            limiter_gr_db: 0.0,
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
            && self.skip_exclusive_track != Some(track.id)
            && self.effective_bit_perfect().applies_to(track.mqa)
            && !matches!(track.format, F::Dsf | F::Dff | F::SacdIso | F::Unknown)
    }

    pub fn set_dsd_story(&mut self, story: DsdStory) {
        self.dsd_story = story;
    }

    pub fn set_dsd_devices(&mut self, devices: Vec<String>) {
        self.dsd_devices = devices;
    }

    pub fn set_quality_mode(&mut self, mode: QualityMode) {
        if mode == self.quality {
            return;
        }
        self.quality = mode;
        // A track that is loaded is re-opened so the mode applies now.
        let live = self.active.is_some()
            && matches!(self.status, PlayerStatus::Playing | PlayerStatus::Paused);
        if live {
            let pos = self.position_ms();
            self.seek_ms(pos);
        }
    }

    /// The user's own processing that exclusive output would bypass right
    /// now (names for the UI). Empty means exclusive output costs nothing.
    pub fn exclusive_blockers(&self) -> Vec<&'static str> {
        let mut b = Vec::new();
        if self.crossfeed.settings().enabled {
            b.push(BLOCKER_CROSSFEED);
        }
        // An enabled EQ with only flat bands changes nothing, so it does not
        // count.
        if self.eq.enabled() && self.eq.bands().iter().any(|band| band.gain_db.abs() > 0.05) {
            b.push(BLOCKER_EQ);
        }
        if self.loudness.enabled() {
            b.push(BLOCKER_LOUDNESS);
        }
        if self.analog.settings().enabled {
            b.push(BLOCKER_ANALOG);
        }
        if self.limiter_enabled {
            b.push(BLOCKER_LIMITER);
        }
        if self.volume < 0.999 {
            b.push(BLOCKER_VOLUME);
        }
        b
    }

    /// Should an *Auto* setting go for the exclusive / native path right
    /// now? Only in Best quality, on an external DAC, with none of the
    /// user's processing that it would bypass.
    fn auto_wants_exclusive(&self) -> bool {
        // Cheapest checks first: this runs on every processing tweak, and asking
        // the OS about the device costs more than looking at our own settings.
        self.quality == QualityMode::Best
            && self.exclusive_blockers().is_empty()
            && self.sink.output_is_external_dac()
    }

    /// The story actually applied. `Auto` follows the quality mode: native
    /// DoP only in Best quality, for a device known (built in, or confirmed
    /// by the user) to decode DoP, when nothing of the user's would be
    /// bypassed; otherwise the FLAC conversion.
    fn effective_dsd_story(&self) -> DsdStory {
        match self.dsd_story {
            DsdStory::Auto => {
                let known = self
                    .sink
                    .output_device_name()
                    .map(|n| crate::dsd_devices::is_known_dsd_device(&n, &self.dsd_devices))
                    .unwrap_or(false);
                if known && self.auto_wants_exclusive() {
                    DsdStory::Native
                } else {
                    DsdStory::Convert
                }
            }
            s => s,
        }
    }

    /// The bit-perfect mode actually applied (`Auto` resolved as above).
    fn effective_bit_perfect(&self) -> BitPerfect {
        match self.bit_perfect {
            BitPerfect::Auto if self.auto_wants_exclusive() => BitPerfect::All,
            BitPerfect::Auto => BitPerfect::Off,
            m => m,
        }
    }

    /// Add a line to this track's notice (they can stack: a missing device
    /// and a DoP fallback are both worth saying).
    fn add_notice(&mut self, msg: String) {
        self.notice = Some(match self.notice.take() {
            Some(prev) => format!("{prev} {msg}"),
            None => msg,
        });
    }

    /// The chosen output device isn't connected and the sink fell back to
    /// another one: say so, so audio doesn't just move to other speakers.
    fn missing_device_notice(&self) -> Option<String> {
        let chosen = self.chosen_device.as_deref()?;
        let actual = self.sink.output_device_name()?;
        (chosen.trim().to_lowercase() != actual.trim().to_lowercase()).then(|| {
            format!(
                "“{}” isn't connected, so this is playing on “{}”.",
                chosen.trim(),
                actual.trim()
            )
        })
    }

    /// An exclusive output failed mid-track (the device went away, or stopped
    /// draining). Carry on from the same spot on shared output rather than
    /// stopping, and say what happened.
    fn recover_from_exclusive_loss(&mut self, what: &str) {
        let Some(id) = self.queue.current().map(|t| t.id) else {
            self.fail(what);
            return;
        };
        let pos = self.position_ms();
        tracing::warn!(track_id = id, %what, "exclusive output lost; continuing on shared output");
        self.pending_notice = Some(format!(
            "The exclusive output stopped ({what}); continuing on shared output."
        ));
        self.skip_exclusive_track = Some(id);
        self.active = None;
        let _ = self.sink.stop();
        self.open_current(Some(pos));
    }

    /// Why a Best-quality Auto setting stayed on shared output for this
    /// track, if it did: an external DAC is selected but the user's own
    /// processing is on.
    fn yielded_to_processing(&self) -> Option<String> {
        let auto_involved =
            self.bit_perfect == BitPerfect::Auto || self.dsd_story == DsdStory::Auto;
        let blockers = self.exclusive_blockers();
        (self.quality == QualityMode::Best
            && auto_involved
            && self.sink.output_is_external_dac()
            && !blockers.is_empty())
        .then(|| {
            format!(
                "Best quality is paused: {} on. Turn {} off for bit-perfect output.",
                blockers.join(" and "),
                if blockers.len() == 1 { "it" } else { "them" }
            )
        })
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
        self.skip_exclusive_track = None;
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

    /// After the queue changed under a playing stream: if the server already
    /// chained a *different* next track into it (gapless), what would play next
    /// is stale, so re-open once at the current position. Otherwise the stream
    /// is left completely alone.
    fn refresh_chain_if_stale(&mut self) {
        let Some(chained) = self.active.as_ref().and_then(|a| a.chained_next_id()) else {
            return;
        };
        if self.queue.peek_next().map(|t| t.id) != Some(chained) {
            self.reopen_live();
        }
    }

    /// Move a queue entry in place. Playback is not interrupted: the current
    /// track keeps playing from where it is (no stream is re-opened unless a
    /// gapless chain was invalidated).
    pub fn move_queue_item(&mut self, from: usize, to: usize) {
        if !self.queue.move_track(from, to) {
            return;
        }
        self.persist_queue();
        self.refresh_chain_if_stale();
    }

    /// Remove a queue entry in place. Removing the playing track moves on to
    /// the one that slides into its place; removing anything else does not
    /// disturb playback.
    pub fn remove_queue_item(&mut self, index: usize) {
        use crate::queue::Removed;
        match self.queue.remove_at(index) {
            None => {}
            Some(Removed::Other) => {
                self.persist_queue();
                self.refresh_chain_if_stale();
            }
            Some(Removed::Current) => {
                self.persist_queue();
                if self.queue.current().is_some() {
                    let paused = self.status == PlayerStatus::Paused;
                    self.open_current(None);
                    if paused && self.status == PlayerStatus::Playing {
                        self.status = PlayerStatus::Paused;
                        let _ = self.sink.pause();
                    }
                } else {
                    self.stop();
                }
            }
        }
    }

    /// Re-open the loaded track at its current position (paused stays paused)
    /// so a change to how it is streamed takes effect now, not at the next open.
    fn reopen_live(&mut self) {
        let live = self.active.is_some()
            && matches!(self.status, PlayerStatus::Playing | PlayerStatus::Paused);
        if live {
            let pos = self.position_ms();
            self.seek_ms(pos);
        }
    }

    pub fn set_global_format(&mut self, fmt: Option<StreamFormat>) {
        if fmt == self.global_format {
            return;
        }
        self.global_format = fmt;
        self.reopen_live();
    }

    pub fn set_track_format(&mut self, track_id: i64, fmt: Option<StreamFormat>) {
        let before = self.track_formats.get(&track_id).copied();
        match fmt {
            Some(f) => {
                self.track_formats.insert(track_id, f);
            }
            None => {
                self.track_formats.remove(&track_id);
            }
        }
        // Only the track that is loaded can be affected right now.
        if before != fmt && self.queue.current().map(|t| t.id) == Some(track_id) {
            self.reopen_live();
        }
    }

    /// Your own processing changed (EQ, loudness, analog stage, volume). If that
    /// flips whether Best quality can go exclusive, re-open the loaded track at
    /// its current position so the switch happens now: turn off the last thing
    /// holding it back and bit-perfect engages; turn something on and it steps
    /// aside. Settings that don't flip the answer (a slider tweak) change nothing.
    fn reconsider_exclusive(&mut self) {
        let is_dsd = matches!(
            self.queue.current().map(|t| t.format),
            Some(AudioFormat::Dsf | AudioFormat::Dff)
        );
        // Only a setting that follows the mode, and applies to this track, cares.
        let follows_mode =
            self.bit_perfect == BitPerfect::Auto || (is_dsd && self.dsd_story == DsdStory::Auto);
        if follows_mode && self.auto_wants_exclusive() != self.exclusive_decision {
            self.reopen_live();
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.reconsider_exclusive();
    }

    /// Route output to a named device (`None` = system default). If a
    /// track is loaded the stream is re-opened on the new device at the
    /// current audible position, keeping paused/playing as it was.
    pub fn set_output_device(&mut self, name: Option<String>) {
        self.chosen_device = name.clone();
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
        self.eq.set_bands(bands)?;
        self.reconsider_exclusive();
        Ok(())
    }

    pub fn set_eq_enabled(&mut self, enabled: bool) {
        self.eq.set_enabled(enabled);
        self.reconsider_exclusive();
    }

    /// Analog character stage (PCM shared path only; DoP and bit-perfect
    /// never call it).
    pub fn set_analog(&mut self, settings: AnalogSettings) {
        if settings != self.analog.settings() {
            // A new setting means a new level: start the reading over.
            self.meter_in.reset();
            self.meter_out.reset();
        }
        self.analog.set_settings(settings);
        self.reconsider_exclusive();
    }

    /// Headphone crossfeed (PCM shared path only). DSP like the others, so
    /// while it is on, Auto does not choose exclusive output.
    pub fn set_crossfeed(&mut self, settings: CrossfeedSettings) {
        self.crossfeed.set_settings(settings);
        self.reconsider_exclusive();
    }

    pub fn set_loudness_enabled(&mut self, enabled: bool) {
        self.loudness.set_enabled(enabled);
        self.reconsider_exclusive();
    }

    /// Turning the limiter off leaves `headroom_guard` as the only protection.
    /// It is DSP like EQ/loudness/analog, so it blocks exclusive output the
    /// same way (see `exclusive_blockers`). Whatever the limiter is still
    /// holding is drained by `pump_pcm` on the next chunk, so flipping this
    /// mid-track never strands buffered audio.
    pub fn set_limiter_enabled(&mut self, enabled: bool) {
        self.limiter_enabled = enabled;
        self.reconsider_exclusive();
    }

    /// Gain reduction to show on the meter, in dB. `None` when the limiter is
    /// off, or when nothing is playing on the shared path (exclusive output
    /// bypasses it) — the UI shows "bypassed" rather than a stale zero.
    fn limiter_gr(&self) -> Option<f32> {
        if !self.limiter_enabled || self.output_path != OutputPath::Pcm {
            return None;
        }
        self.active.as_ref().map(|_| self.limiter_gr_db)
    }

    pub fn set_loudness_target(&mut self, lufs: f32) {
        self.loudness.set_target(lufs);
    }

    /// The analog stage's effect on the level, once about a second of audio is
    /// behind it. Reported for the dry slot too (delta 0 by construction), so
    /// A/B level-matching has a live peak reading on both sides.
    fn analog_level(&self) -> Option<AnalogLevel> {
        if self.meter_in.seconds() < 1.0 {
            return None;
        }
        let (i, o) = (self.meter_in.lufs()?, self.meter_out.lufs()?);
        Some(AnalogLevel {
            input_lufs: i,
            output_lufs: o,
            delta_db: o - i,
            peak_dbfs: 20.0 * self.peak_out.max(1e-6).log10(),
            seconds: self.meter_in.seconds(),
        })
    }

    /// Current DSP settings, for the shell to persist and the UI to mirror.
    pub fn dsp_settings(&self) -> DspSettings {
        DspSettings {
            eq_bands: self.eq.bands().to_vec(),
            eq_enabled: self.eq.enabled(),
            loudness_enabled: self.loudness.enabled(),
            loudness_target: self.loudness.target(),
            analog: self.analog.settings(),
            limiter_enabled: self.limiter_enabled,
            crossfeed: self.crossfeed.settings(),
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
        self.exclusive_decision = self.auto_wants_exclusive();
        self.notice = self.pending_notice.take();
        if let Some(m) = self.missing_device_notice() {
            self.add_notice(m);
        }
        if let Some(m) = self.yielded_to_processing() {
            self.add_notice(m);
        }

        let mut fmt = resolve_format(
            &track,
            self.global_format,
            self.track_formats.get(&track.id).copied(),
            self.effective_dsd_story(),
        );

        // DoP fallback chain: any reason DoP can't start (no exclusive sink,
        // rate refused, hog mode taken, format refused, stream error) drops
        // this track to the FLAC transcode and says why. Playback never
        // fails outright for a device that can do PCM.
        if fmt == StreamFormat::Dop && self.skip_exclusive_track == Some(track.id) {
            fmt = StreamFormat::Flac;
        }
        if fmt == StreamFormat::Dop {
            match self.try_dop(&track, seek) {
                Ok(()) => return,
                Err(why) => {
                    tracing::warn!(track_id = track.id, %why, "DoP unavailable; falling back to FLAC");
                    self.add_notice(format!("Played as PCM (FLAC): {why}."));
                    self.active = None;
                    fmt = StreamFormat::Flac;
                }
            }
        }

        self.sink.select_output_path(OutputPath::Pcm);
        self.output_path = OutputPath::Pcm;
        self.open_pcm(&track, seek, fmt);
    }

    /// Start DoP for `track`, or say why it cannot. On `Err` the sink has
    /// been released and nothing is playing.
    fn try_dop(&mut self, track: &Track, seek: Option<u64>) -> Result<(), String> {
        if !self.sink.supports_dop() {
            return Err("this output can't play DSD natively".into());
        }
        if self.dop_capable_rate(track).is_none() {
            return Err("your DAC doesn't accept the sample rate this DSD file needs".into());
        }
        self.sink.select_output_path(OutputPath::Dop);
        self.output_path = OutputPath::Dop;
        let result = self.open_dop(track, seek);
        if result.is_err() {
            let _ = self.sink.stop();
        }
        result
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
    fn open_dop(&mut self, track: &Track, seek: Option<u64>) -> Result<(), String> {
        let dop_rate = match self.dop_capable_rate(track) {
            Some(r) => r,
            None => {
                return Err("your DAC doesn't accept the sample rate this DSD file needs".into())
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
                tracing::warn!(error = %e, "DoP stream request failed");
                return Err("the server couldn't provide the DSD stream".into());
            }
        };
        let gapless_mode = info.gapless_mode.clone();
        let progress = info.progress.clone();
        let chained = gapless_mode.as_deref() == Some("chained");
        let established = self.dop_spec;
        let stream = match DopStream::new(info.reader, dop_rate, established, chained) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "DoP stream unreadable");
                return Err("the DSD stream couldn't be read".into());
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

        if let Err(e) = self.sink.open(track) {
            tracing::warn!(error = %e, "DoP sink open failed");
            return Err(
                "your DAC couldn't be set up for native DSD (another app may be using it)".into(),
            );
        }
        if let Err(e) = self.sink.play() {
            tracing::warn!(error = %e, "DoP sink start failed");
            return Err("your DAC didn't start".into());
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
        Ok(())
    }

    fn open_pcm(&mut self, track: &Track, seek: Option<u64>, fmt: StreamFormat) {
        debug_assert_ne!(fmt, StreamFormat::Dop);
        let want_bp = self.wants_bit_perfect(track, fmt);
        // Bit-perfect plays one track per stream: the exclusive device is
        // opened at each file's own rate, so the server must not chain the
        // next track (whose rate may differ) into this response.
        // The same goes for any next track whose output rate or channel count
        // differs: the sink, resampler, EQ and meters are set up once per stream.
        let next_id = if want_bp {
            None
        } else {
            self.queue
                .peek_next()
                .filter(|next| can_chain(track, next, fmt))
                .map(|t| t.id)
        };

        // Loudness pre-scan (v1): one extra deterministic stream per
        // first-play of a track; the gain is cached by (track, format).
        // Cost: double LAN bandwidth + a second server transcode for
        // uncached tracks. DoP never reaches this path.
        let loudness_gain_db = if self.loudness.enabled() && !want_bp {
            let track_id = track.id;
            let transport = &*self.transport;
            // Plan the gain against the track's peak and the EQ's worst-case
            // boost, so the result cannot clip at the output.
            let eq_boost = self.eq.max_boost_db();
            self.loudness.gain_for_levels(track_id, fmt, eq_boost, || {
                scan_track_levels(transport, track_id, fmt)
            })
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
                tracing::warn!(error = %e, "stream request failed");
                self.fail("Couldn't get the stream from the server.");
                return;
            }
        };
        let gapless_mode = info.gapless_mode.clone();
        let progress = info.progress.clone();
        let expect_chained = gapless_mode.as_deref() == Some("chained");
        let decoder = match StreamDecoder::new(info.reader, expect_chained) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "decode failed");
                self.fail("Couldn't decode this file.");
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
            let mut why: Option<String> = None;
            let rate_ok = self.sink.exclusive_pcm_rate(spec.sample_rate) == Some(spec.sample_rate);
            if !rate_ok {
                tracing::warn!(
                    track_id = track.id,
                    rate = spec.sample_rate,
                    "bit-perfect: output device cannot take this rate exclusively; using shared output"
                );
                why = Some(format!(
                    "the output device doesn't offer {} kHz",
                    spec.sample_rate as f64 / 1000.0
                ));
            } else if spec.channels == 0 || spec.channels > 8 {
                tracing::warn!(
                    track_id = track.id,
                    "bit-perfect: unsupported channel count; using shared output"
                );
                why = Some("the channel count isn't supported for exclusive output".into());
            } else {
                match self
                    .sink
                    .open_exclusive_pcm(spec.sample_rate, spec.channels as u16)
                {
                    Ok(()) => bit_perfect = true,
                    Err(e) => {
                        tracing::warn!(
                            track_id = track.id,
                            error = %e,
                            "bit-perfect: could not open the exclusive device; using shared output"
                        );
                        why = Some("your DAC couldn't be opened for exclusive use (another app may be using it)".into());
                    }
                }
            }
            if !bit_perfect {
                self.sink.select_output_path(OutputPath::Pcm);
                if let Some(w) = why {
                    self.add_notice(format!("Played on shared output: {w}."));
                }
            }
        }
        // Opened exclusively but won't start (seen with a USB DAC): fall back
        // to shared output, as for a device that won't open, rather than
        // failing the track.
        if bit_perfect {
            if let Err(e) = self.sink.play() {
                tracing::error!(
                    track_id = track.id,
                    error = %e,
                    "bit-perfect: exclusive output would not start; using shared output"
                );
                let _ = self.sink.stop();
                bit_perfect = false;
                self.sink.select_output_path(OutputPath::Pcm);
                self.add_notice(
                    "Played on shared output: your DAC couldn't be started for exclusive use \
                     (another app may be using it)."
                        .to_string(),
                );
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
            // Untouched: native rate, no resampler. Already started above.
            sink_rate = spec.sample_rate;
            resampler = None;
        } else {
            // Open the sink before the resample decision: C2's cpal sink
            // negotiates the device rate in open(), so the query below sees
            // the real rate (native when the device takes it).
            if let Err(e) = self.sink.open(track) {
                tracing::error!(track_id = track.id, error = %e, "shared output would not open");
                self.fail("Couldn't open the audio output.");
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
            if let Err(e) = self.sink.play() {
                tracing::error!(track_id = track.id, error = %e, "shared output would not start");
                self.fail("Couldn't start the audio output.");
                return;
            }
        }

        // The EQ runs on what the sink receives (after any resampling), so
        // it is designed at the sink rate, not the file's.
        self.eq.set_sample_rate(sink_rate);
        self.crossfeed.prepare(sink_rate);
        self.crossfeed.reset();
        self.analog.prepare(sink_rate);
        self.meter_in.set_sample_rate(sink_rate);
        self.meter_out.set_sample_rate(sink_rate);
        self.limiter.prepare(sink_rate);
        self.peak_out = 0.0;

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
                    tracing::warn!(error = %e, "decode failed");
                    self.fail("Couldn't decode this file.");
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
                    self.fail("This file produced no audio.");
                    return;
                }
            }
            self.flush_limiter_tail();
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
                self.recover_from_exclusive_loss("the device stopped accepting audio");
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
        // v1 DSP chain, fixed order: crossfeed -> EQ -> analog -> loudness
        // gain (ramped) -> volume -> look-ahead limiter -> headroom guard
        // (belt and suspenders; a no-op once the limiter holds the ceiling).
        // Crossfeed first: it models speakers, and the EQ corrects the
        // headphones, so the EQ shapes what the ear will actually receive.
        let mut chunk: Vec<f32> = out.to_vec();
        self.crossfeed.process(&mut chunk, channels);
        self.eq.process(&mut chunk, channels);
        // Meter continuously, dry or wet: A/B level-matching needs a reading
        // on the dry slot too, not just while the stage is processing.
        // "Settled" excludes only the fade transition itself, on either side.
        let settled_before = !self.analog.is_active() || self.analog.is_steady();
        let ms_in = self.meter_in.measure(&chunk, channels);
        self.analog.process(&mut chunk, channels);
        let settled_after = !self.analog.is_active() || self.analog.is_steady();
        let ms_out = self.meter_out.measure(&chunk, channels);
        let frames = chunk.len() / channels;
        // Both meters integrate the same chunks (gated on the input), so
        // their difference is the stage's effect on the level.
        if settled_before && settled_after && ms_in >= 1.174e-7 {
            self.meter_in.integrate(ms_in, frames);
            self.meter_out.integrate(ms_out, frames);
        }
        let dt = frames as f32 / active.sink_rate.max(1) as f32;
        let peak = chunk.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        self.peak_out = peak.max(self.peak_out * 0.5f32.powf(dt));
        self.gain_ramp.apply(&mut chunk);
        let vol = self.volume;
        if vol < 0.999 {
            for s in chunk.iter_mut() {
                *s *= vol;
            }
        }
        // EQ boosts and loudness gain can lift peaks past full scale, which the
        // output device would hard-clip. The limiter ducks ahead of a peak so
        // the ceiling is reached transparently; the guard is a cheap backstop.
        // The limiter's block length can differ from what went in (shorter,
        // right after a reset, while its look-ahead window fills; see its
        // docs), so `written_frames` below - not the pre-limiter frame count -
        // is what actually reaches the sink.
        if self.limiter_enabled {
            self.limiter.process(&mut chunk, channels);
            // Meter ballistics, same shape as `peak_out`: hold the deepest
            // reduction of this chunk, then fall back about 20 dB/s, so a
            // 5 ms duck is still on screen at the next snapshot.
            let dt = (chunk.len() / channels.max(1)) as f32 / active.sink_rate.max(1) as f32;
            let gr = self.limiter.take_reduction_db();
            self.limiter_gr_db = gr
                .max(self.limiter_gr_db - GR_DECAY_DB_PER_SEC * dt)
                .max(0.0);
        } else {
            self.limiter_gr_db = 0.0;
            // Off: drain anything it was still holding in front of this chunk,
            // so switching it off mid-track loses no audio. A no-op (empty,
            // already reset) on every chunk after the first.
            let held = self.limiter.flush();
            if !held.is_empty() {
                chunk.splice(0..0, held);
            }
        }
        headroom_guard(&mut chunk);
        let written_frames = chunk.len() / channels;

        let sink_rate = active.sink_rate;
        let ch = active.spec.channels;
        if !chunk.is_empty()
            && self
                .sink
                .write(PcmChunk {
                    frames: chunk,
                    sample_rate: sink_rate,
                    channels: ch as u8,
                })
                .is_err()
        {
            self.fail("The audio output stopped responding.");
            return;
        }
        let active = match self.active.as_mut() {
            Some(ActiveStream::Pcm(a)) => a,
            _ => return,
        };
        active.pumped_frames += written_frames as u64;
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
                    tracing::warn!(error = %e, "DoP read failed");
                    self.fail("Couldn't read the DSD stream.");
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
                    self.fail("The DSD stream contained no audio.");
                    return;
                }
            }
            self.on_stream_end();
            return;
        }
        self.empty_streak = 0;
        if self.sink.write_dop(&buf[..n]).is_err() {
            self.recover_from_exclusive_loss("the device stopped accepting DSD audio");
            return;
        }
        if let Some(ActiveStream::Dop(a)) = self.active.as_mut() {
            a.pumped_frames += (n / frame_bytes) as u64;
            a.advance_display();
        }
    }

    /// Writes out whatever the look-ahead limiter is still holding. Called
    /// once a `PcmStream`'s decoder is genuinely exhausted (`pump_pcm`,
    /// `decoded_frames == 0`) — never on an internal chained-gapless segment
    /// boundary, where the same decoder keeps producing continuous audio and
    /// the limiter must keep running uninterrupted. Bit-perfect streams never
    /// touch the limiter, so there is nothing to flush for them.
    fn flush_limiter_tail(&mut self) {
        let active = match self.active.as_ref() {
            Some(ActiveStream::Pcm(a)) if !a.bit_perfect => a,
            _ => return,
        };
        let (sink_rate, channels) = (active.sink_rate, active.spec.channels);
        let tail = self.limiter.flush();
        if tail.is_empty() {
            return;
        }
        let frames = tail.len() / channels as usize;
        if self
            .sink
            .write(PcmChunk {
                frames: tail,
                sample_rate: sink_rate,
                channels: channels as u8,
            })
            .is_err()
        {
            return; // the stream is ending anyway; nothing left to recover
        }
        if let Some(ActiveStream::Pcm(a)) = self.active.as_mut() {
            a.pumped_frames += frames as u64;
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
        let (track, position_ms, duration_ms, buffered_ms, output_rate_hz, format, chain) =
            match &self.active {
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
            analog_level: if self.active.is_some() && self.output_path == OutputPath::Pcm {
                self.analog_level()
            } else {
                None
            },
            limiter_gr_db: self.limiter_gr(),
            format,
            chain,
            output_path: self.output_path,
            volume: self.volume,
            error: self.error.clone(),
            notice: self.notice.clone(),
            exclusive_blockers: self
                .exclusive_blockers()
                .into_iter()
                .map(String::from)
                .collect(),
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

/// The rate a track comes out at once the server has shaped it for `fmt`
/// (mirrors the server's `describe_target`): Opus is always 48 kHz, MP3 tops
/// out at 48 kHz, everything else keeps the file's rate. `None` = unknown.
fn output_rate(track: &Track, fmt: StreamFormat) -> Option<u32> {
    match fmt {
        StreamFormat::Opus => Some(48_000),
        StreamFormat::Mp3 => track.sample_rate.map(|r| r.min(48_000)),
        _ => track.sample_rate,
    }
}

/// Whether `next` may be chained into the response for `current`. A chained
/// response is decoded, resampled and metered against the *first* track's
/// spec, so a rate or channel change mid-response would play the second track
/// at the wrong speed or with scrambled channels. Unknown metadata is given
/// the benefit of the doubt (the previous behaviour).
fn can_chain(current: &Track, next: &Track, fmt: StreamFormat) -> bool {
    let same_rate = match (output_rate(current, fmt), output_rate(next, fmt)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    let same_channels = match (current.channels, next.channels) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    same_rate && same_channels
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
    /// Move a queue entry (list positions) without interrupting playback.
    MoveQueueItem(usize, usize),
    /// Remove a queue entry (list position); moves on only if it was playing.
    RemoveQueueItem(usize),
    Pause,
    Resume,
    Toggle,
    Stop,
    Seek(u64),
    Next,
    Prev,
    SetGlobalFormat(Option<StreamFormat>),
    /// Devices the user confirmed as DoP-capable (Settings).
    SetDsdDevices(Vec<String>),
    /// Top-level quality mode (Settings).
    SetQualityMode(QualityMode),
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
    SetLimiterEnabled(bool),
    SetCrossfeed(CrossfeedSettings),
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
    /// Client DSD preference (Settings → DSD handling). Defaults to `Auto`:
    /// native DoP on known DSD-capable DACs, FLAC conversion elsewhere.
    #[serde(default)]
    dsd_story: DsdStory,
    /// Output devices the user confirmed as DoP-capable (used by `Auto`).
    #[serde(default)]
    dsd_devices: Vec<String>,
    /// Top-level quality mode (Best quality / Compatible). The fine controls
    /// below default to Auto, which follows it.
    #[serde(default)]
    quality_mode: QualityMode,
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
    /// Last volume (0..=1), restored at launch. `None`: never set (the
    /// engine's default).
    #[serde(default)]
    volume: Option<f32>,
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
        let text = std::fs::read_to_string(path).ok();
        let mut settings: Self = text
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Self {
                server_url: DEFAULT_SERVER_URL.to_string(),
                dsp: DspSettings::default(),
                dsd_story: DsdStory::default(),
                dsd_devices: Vec::new(),
                quality_mode: QualityMode::default(),
                global_format: None,
                output_device: None,
                bit_perfect: BitPerfect::default(),
                volume: None,
            });
        // One-time migration: a file written before the quality mode existed
        // carries the old individual defaults (or choices made while
        // troubleshooting). Reset the fine controls to Auto so the mode
        // governs them; explicit overrides can be set again in Advanced.
        let pre_quality = text
            .as_deref()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .map(|v| v.get("quality_mode").is_none())
            .unwrap_or(false);
        if pre_quality {
            settings.dsd_story = DsdStory::Auto;
            settings.bit_perfect = BitPerfect::Auto;
            settings.global_format = None;
            settings.save(path);
        }
        settings
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
    dsd_devices: Arc<RwLock<Vec<String>>>,
    quality_mode: Arc<RwLock<QualityMode>>,
    output_device: Arc<RwLock<Option<String>>>,
    bit_perfect: Arc<RwLock<BitPerfect>>,
    global_format: Arc<RwLock<Option<StreamFormat>>>,
    settings_path: PathBuf,
    /// The latest volume not yet written to the settings file. Dragging the
    /// slider sends many changes a second, so a saver thread writes the
    /// last one at most every [`VOLUME_SAVE_EVERY`] (and shutdown flushes).
    volume_to_save: Arc<Mutex<Option<f32>>>,
    /// Signalled by the playback thread once it has saved and is about to
    /// exit; taken by the first [`shutdown`](Self::shutdown).
    stopped: Mutex<Option<mpsc::Receiver<()>>>,
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
        ctrl.send(EngineCommand::SetLimiterEnabled(dsp.limiter_enabled));
        ctrl.send(EngineCommand::SetCrossfeed(dsp.crossfeed));
        // Playback preferences (persisted; default preserves pre-C3 behavior).
        ctrl.send(EngineCommand::SetDsdStory(settings.dsd_story));
        ctrl.send(EngineCommand::SetGlobalFormat(settings.global_format));
        *ctrl.dsd_story.write().expect("dsd lock") = settings.dsd_story;
        ctrl.send(EngineCommand::SetQualityMode(settings.quality_mode));
        *ctrl.quality_mode.write().expect("quality lock") = settings.quality_mode;
        ctrl.send(EngineCommand::SetDsdDevices(settings.dsd_devices.clone()));
        *ctrl.dsd_devices.write().expect("dsd devices lock") = settings.dsd_devices.clone();
        // Output device (before any playback opens the sink).
        ctrl.send(EngineCommand::SetOutputDevice(
            settings.output_device.clone(),
        ));
        *ctrl.output_device.write().expect("device lock") = settings.output_device.clone();
        ctrl.send(EngineCommand::SetBitPerfect(settings.bit_perfect));
        *ctrl.bit_perfect.write().expect("bit-perfect lock") = settings.bit_perfect;
        *ctrl.global_format.write().expect("format lock") = settings.global_format;
        if let Some(v) = settings.volume {
            ctrl.send(EngineCommand::SetVolume(v));
        }
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
        let (stopped_tx, stopped_rx) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("player-playback".into())
            .spawn(move || {
                playback_loop(rx, player, snap2, ev2);
                let _ = stopped_tx.send(());
            })
            .expect("spawn playback thread");

        let volume_to_save = Arc::new(Mutex::new(None));
        spawn_volume_saver(Arc::downgrade(&volume_to_save), settings_path.clone());
        Self {
            tx,
            snapshot,
            events,
            volume_to_save,
            server_url,
            dsd_story: Arc::new(RwLock::new(DsdStory::default())),
            dsd_devices: Arc::new(RwLock::new(Vec::new())),
            quality_mode: Arc::new(RwLock::new(QualityMode::default())),
            output_device: Arc::new(RwLock::new(None)),
            bit_perfect: Arc::new(RwLock::new(BitPerfect::default())),
            global_format: Arc::new(RwLock::new(None)),
            settings_path,
            stopped: Mutex::new(Some(stopped_rx)),
            _thread: thread,
        }
    }

    fn send(&self, cmd: EngineCommand) {
        let _ = self.tx.send(cmd);
    }

    pub fn play_queue(&self, tracks: Vec<Track>, index: usize) {
        self.send(EngineCommand::PlayQueue(tracks, index));
    }
    /// Reorder the queue in place; playback continues uninterrupted.
    pub fn move_queue_item(&self, from: usize, to: usize) {
        self.send(EngineCommand::MoveQueueItem(from, to));
    }
    /// Remove a queue entry in place; playback continues unless it was playing.
    pub fn remove_queue_item(&self, index: usize) {
        self.send(EngineCommand::RemoveQueueItem(index));
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

    /// Top-level quality mode. Persisted; a playing track re-opens under it.
    pub fn set_quality_mode(&self, mode: QualityMode) {
        *self.quality_mode.write().expect("quality lock") = mode;
        let mut settings = EngineSettings::load(&self.settings_path);
        settings.quality_mode = mode;
        settings.save(&self.settings_path);
        self.send(EngineCommand::SetQualityMode(mode));
    }

    pub fn quality_mode(&self) -> QualityMode {
        *self.quality_mode.read().expect("quality lock")
    }

    /// Output devices the user confirmed as DoP-capable, for `Auto`.
    pub fn dsd_devices(&self) -> Vec<String> {
        self.dsd_devices.read().expect("dsd devices lock").clone()
    }

    /// Mark (or unmark) an output device as decoding DoP. Persisted; a
    /// playing track keeps its current rendition until the next track.
    pub fn set_dsd_device_confirmed(&self, device: &str, confirmed: bool) {
        let device = device.trim().to_string();
        if device.is_empty() {
            return;
        }
        let mut list = self.dsd_devices.write().expect("dsd devices lock");
        list.retain(|d| !d.trim().eq_ignore_ascii_case(&device));
        if confirmed {
            list.push(device);
        }
        let snapshot = list.clone();
        drop(list);
        let mut settings = EngineSettings::load(&self.settings_path);
        settings.dsd_devices = snapshot.clone();
        settings.save(&self.settings_path);
        self.send(EngineCommand::SetDsdDevices(snapshot));
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
    /// Persisted (batched: see `volume_to_save`).
    pub fn set_volume(&self, v: f32) {
        if v.is_finite() {
            *self.volume_to_save.lock().expect("volume lock") = Some(v.clamp(0.0, 1.0));
        }
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

    /// Clip protection on the shared path. Persisted; off by default, like
    /// loudness normalization.
    pub fn set_limiter_enabled(&self, enabled: bool) {
        self.update_dsp_settings(|dsp| dsp.limiter_enabled = enabled);
        self.send(EngineCommand::SetLimiterEnabled(enabled));
    }

    /// Headphone crossfeed, clamped before it is saved or applied. Values
    /// that aren't finite are ignored.
    pub fn set_crossfeed(&self, settings: CrossfeedSettings) {
        let Some(settings) = settings.clamped() else {
            return;
        };
        self.update_dsp_settings(|dsp| dsp.crossfeed = settings);
        self.send(EngineCommand::SetCrossfeed(settings));
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

impl EngineController {
    /// Stop the playback thread and wait (up to [`SHUTDOWN_WAIT`]) for it to
    /// save the queue and live playhead. Call this when the app is quitting:
    /// the periodic save only runs every 5 s, so without it a quit loses up
    /// to that much position. Safe to call more than once.
    pub fn shutdown(&self) {
        save_volume(&self.volume_to_save, &self.settings_path);
        let _ = self.tx.send(EngineCommand::Shutdown);
        let rx = self.stopped.lock().expect("stopped lock").take();
        if let Some(rx) = rx {
            let _ = rx.recv_timeout(SHUTDOWN_WAIT);
        }
    }
}

/// How often, at most, a volume change is written to the settings file.
const VOLUME_SAVE_EVERY: Duration = Duration::from_millis(400);

/// Write the pending volume, if any, into the settings file.
fn save_volume(pending: &Mutex<Option<f32>>, settings_path: &PathBuf) {
    let Some(v) = pending.lock().expect("volume lock").take() else {
        return;
    };
    let mut settings = EngineSettings::load(settings_path);
    settings.volume = Some(v);
    settings.save(settings_path);
}

/// Writes pending volume changes every [`VOLUME_SAVE_EVERY`]; exits once
/// the controller (the only strong owner of `pending`) is gone.
fn spawn_volume_saver(pending: std::sync::Weak<Mutex<Option<f32>>>, settings_path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("volume-saver".into())
        .spawn(move || loop {
            std::thread::sleep(VOLUME_SAVE_EVERY);
            let Some(pending) = pending.upgrade() else {
                break;
            };
            save_volume(&pending, &settings_path);
        });
}

/// How long [`EngineController::shutdown`] waits for the final save. The
/// thread only has to write one small JSON file; this bounds a wedged sink.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

impl Drop for EngineController {
    fn drop(&mut self) {
        self.shutdown();
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
        if player.status() == PlayerStatus::Playing
            && last_saved.elapsed() >= Duration::from_secs(5)
        {
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
    // Shutting down (quit, or the controller was dropped): save the live
    // playhead now rather than leaving up to 5 s of it to the periodic save.
    // A stopped player has no position, so this is harmless when idle.
    player.persist_position();
}

/// Would the playback thread publish `b` after `a`? (The UI-visible identity
/// differs.) Exposed so tests can check that a change reaches the UI on its own.
pub fn snapshot_key_differs(a: &PlayerSnapshot, b: &PlayerSnapshot) -> bool {
    snapshot_key(a) != snapshot_key(b)
}

/// Identity of the UI-visible parts of a [`PlayerSnapshot`]; see [`snapshot_key`].
type SnapshotKey = (
    PlayerStatus,
    Option<i64>,
    Vec<i64>,
    RepeatMode,
    bool,
    u32,
    u64,
    OutputPath,
    Vec<String>,
    Option<String>,
);

/// UI-visible snapshot identity minus the ever-moving playhead.
fn snapshot_key(s: &PlayerSnapshot) -> SnapshotKey {
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
        // Which of the user's processing holds Best quality back, and the
        // notice explaining a fallback: both must reach the UI on their own,
        // even while paused or stopped.
        s.exclusive_blockers.clone(),
        s.notice.clone(),
    )
}

fn apply_command(player: &mut Player, cmd: EngineCommand) {
    match cmd {
        EngineCommand::PlayQueue(tracks, index) => player.play_queue(tracks, index),
        EngineCommand::MoveQueueItem(from, to) => player.move_queue_item(from, to),
        EngineCommand::RemoveQueueItem(i) => player.remove_queue_item(i),
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
        EngineCommand::SetDsdDevices(d) => player.set_dsd_devices(d),
        EngineCommand::SetQualityMode(m) => player.set_quality_mode(m),
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
        EngineCommand::SetLimiterEnabled(b) => player.set_limiter_enabled(b),
        EngineCommand::SetCrossfeed(c) => player.set_crossfeed(c),
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

#[cfg(test)]
mod can_chain_tests {
    use super::*;
    use kahawai_core::format::AudioFormat;

    fn t(rate: Option<u32>, channels: Option<u8>) -> Track {
        Track {
            id: 1,
            path: "/m/1.flac".into(),
            hash: Some("h".into()),
            format: AudioFormat::Flac,
            sample_rate: rate,
            bit_depth: Some(16),
            channels,
            duration_ms: Some(1000),
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
    fn lossless_chains_only_at_the_same_rate() {
        let a = t(Some(44_100), Some(2));
        assert!(can_chain(&a, &t(Some(44_100), Some(2)), StreamFormat::Flac));
        assert!(!can_chain(
            &a,
            &t(Some(96_000), Some(2)),
            StreamFormat::Flac
        ));
        assert!(!can_chain(
            &a,
            &t(Some(96_000), Some(2)),
            StreamFormat::Passthrough
        ));
    }

    #[test]
    fn opus_and_mp3_output_rates_are_capped_so_mixed_sources_still_chain() {
        let a = t(Some(44_100), Some(2));
        let hi = t(Some(96_000), Some(2));
        assert!(
            can_chain(&a, &hi, StreamFormat::Opus),
            "Opus is always 48 kHz"
        );
        assert!(
            can_chain(&t(Some(48_000), Some(2)), &hi, StreamFormat::Mp3),
            "MP3 caps at 48 kHz"
        );
        assert!(
            !can_chain(&a, &hi, StreamFormat::Mp3),
            "44.1 vs 48 kHz differ"
        );
    }

    #[test]
    fn a_channel_change_never_chains_and_unknowns_are_allowed() {
        let a = t(Some(44_100), Some(2));
        assert!(!can_chain(
            &a,
            &t(Some(44_100), Some(1)),
            StreamFormat::Opus
        ));
        assert!(can_chain(&a, &t(None, None), StreamFormat::Flac));
    }
}
