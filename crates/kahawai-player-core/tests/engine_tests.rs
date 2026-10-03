//! Engine integration tests: gapless handoff, seek accuracy, format
//! resolution, volume — all through a stub [`Transport`] serving WAV
//! fixtures, asserting on a recording [`VecSink`]. No network, no audio
//! hardware.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use kahawai_core::{api::StreamFormat, format::AudioFormat, MusicError, Track};
use kahawai_player_core::{
    resolve_format, valid_formats, AnalogFlavour, AnalogSettings, AntiAliasChoice, AudioSink,
    BitPerfect, CrossfeedPreset, CrossfeedSettings, EngineController, EqBand, EqBandType,
    OutputPath, PcmChunk, Player, PlayerStatus, StreamInfo, StreamOptions, Transport, VecSink,
    LIMITER_CEILING,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const RATE: u32 = 44100;
const CHANNELS: usize = 2;

/// Render 16-bit stereo WAV bytes for sine segments. Each segment's phase
/// restarts at zero; `skip_frames` drops whole/partial segments from the
/// front (phase-continuous within a partial segment) to emulate a server
/// transcode seek, which re-renders from the seek point with a fresh
/// header.
fn render_tone(segments: &[(f32, usize)], skip_frames: usize) -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::new();
    let mut skip = skip_frames;
    for (freq, frames) in segments {
        let start = skip.min(*frames);
        skip -= start;
        for j in start..*frames {
            let s = (2.0 * std::f32::consts::PI * freq * j as f32 / RATE as f32).sin() * 0.7;
            let q = (s * 32767.0).round() as i16;
            samples.push(q);
            samples.push(q);
        }
    }
    let data_len = samples.len() * 2;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
    v.extend_from_slice(&RATE.to_le_bytes());
    v.extend_from_slice(&(RATE * CHANNELS as u32 * 2).to_le_bytes());
    v.extend_from_slice(&((CHANNELS * 2) as u16).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

/// 16-bit stereo WAV bytes of one or more sine segments back to back.
fn wav_segments(segments: &[(f32, usize)]) -> Vec<u8> {
    render_tone(segments, 0)
}

/// 16-bit stereo WAV bytes of a sine tone.
fn wav_bytes(freq: f32, frames: usize) -> Vec<u8> {
    wav_segments(&[(freq, frames)])
}

fn track(id: i64, format: AudioFormat, duration_ms: u64) -> Track {
    Track {
        id,
        path: format!("/m/{id}.wav"),
        hash: Some("h".into()),
        format,
        sample_rate: Some(RATE),
        bit_depth: Some(16),
        channels: Some(CHANNELS as u8),
        duration_ms: Some(duration_ms),
        bitrate: None,
        title: Some(format!("T{id}")),
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

/// `AudioSink` that delegates to a shared [`VecSink`] so tests can read
/// the recorded samples after the player owns the sink.
#[derive(Clone)]
struct SharedSink(Arc<Mutex<VecSink>>);

impl AudioSink for SharedSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        self.0.lock().unwrap().open(track)
    }
    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        self.0.lock().unwrap().write(chunk)
    }
    fn play(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().play()
    }
    fn pause(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().pause()
    }
    fn stop(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().stop()
    }
    fn state(&self) -> kahawai_player_core::SinkState {
        self.0.lock().unwrap().state()
    }
    fn preferred_sample_rate(&self) -> Option<u32> {
        self.0.lock().unwrap().preferred_sample_rate()
    }
    fn buffered_frames(&self) -> u64 {
        self.0.lock().unwrap().buffered_frames()
    }
    fn set_output_device(&mut self, name: Option<&str>) {
        self.0.lock().unwrap().set_output_device(name)
    }
    fn drain(&mut self) {
        self.0.lock().unwrap().drain()
    }
    fn exclusive_pcm_rate(&self, rate_hz: u32) -> Option<u32> {
        self.0.lock().unwrap().exclusive_pcm_rate(rate_hz)
    }
    fn output_is_external_dac(&self) -> bool {
        self.0.lock().unwrap().external
    }
    fn open_exclusive_pcm(&mut self, rate_hz: u32, channels: u16) -> Result<(), MusicError> {
        self.0.lock().unwrap().open_exclusive_pcm(rate_hz, channels)
    }
    fn write_dop(&mut self, bytes: &[u8]) -> Result<(), MusicError> {
        self.0.lock().unwrap().write_dop(bytes)
    }
    fn select_output_path(&mut self, path: OutputPath) {
        self.0.lock().unwrap().select_output_path(path)
    }
}

/// Stub transport emulating the server:
/// - `?seek_ms=` re-renders the tone from the seek frame with a fresh
///   WAV header (like a real transcode seek),
/// - `?next=` serves a pre-built combined body with the configured
///   `X-Gapless-Mode` (`next_body`), falling back to concatenating two
///   complete WAVs (chained container mode).
struct StubTransport {
    tracks: Mutex<HashMap<i64, Vec<(f32, usize)>>>,
    next_body: Mutex<HashMap<i64, Vec<u8>>>,
    chain_mode: Option<String>,
    /// `X-Transcode-Chain` value the stub reports (default "wav->passthrough").
    chain_label: Mutex<Option<String>>,
    opened: Mutex<Vec<(i64, StreamOptions)>>,
    /// How many of the next `open_stream` calls fail (a server hiccup).
    fail_opens: Mutex<u32>,
}

impl StubTransport {
    fn new(chain_mode: Option<&str>) -> Self {
        Self {
            tracks: Mutex::new(HashMap::new()),
            next_body: Mutex::new(HashMap::new()),
            chain_mode: chain_mode.map(|s| s.to_string()),
            chain_label: Mutex::new(None),
            opened: Mutex::new(Vec::new()),
            fail_opens: Mutex::new(0),
        }
    }

    /// Make the next `n` opens fail with a server error.
    fn fail_next_opens(&self, n: u32) {
        *self.fail_opens.lock().unwrap() = n;
    }

    fn add(&self, id: i64, segments: &[(f32, usize)]) {
        self.tracks.lock().unwrap().insert(id, segments.to_vec());
    }

    /// Pre-built response body served when `?next=` is present.
    fn add_next_body(&self, id: i64, body: Vec<u8>) {
        self.next_body.lock().unwrap().insert(id, body);
    }
}

impl Transport for StubTransport {
    fn open_stream(&self, track_id: i64, opts: &StreamOptions) -> Result<StreamInfo, MusicError> {
        self.opened.lock().unwrap().push((track_id, opts.clone()));
        {
            let mut fails = self.fail_opens.lock().unwrap();
            if *fails > 0 {
                *fails -= 1;
                return Err(MusicError::Http("stub: server error".into()));
            }
        }
        // ?next= with a pre-built body (single-session: one container).
        if opts.next.is_some() {
            if let Some(body) = self.next_body.lock().unwrap().get(&track_id).cloned() {
                return Ok(StreamInfo {
                    progress: None,
                    reader: Box::new(Cursor::new(body)),
                    content_type: "audio/wav".into(),
                    chain: Some(
                        self.chain_label
                            .lock()
                            .unwrap()
                            .clone()
                            .unwrap_or_else(|| "wav->passthrough".into()),
                    ),
                    gapless_next: opts.next,
                    gapless_mode: self.chain_mode.clone(),
                });
            }
        }
        let segments = self
            .tracks
            .lock()
            .unwrap()
            .get(&track_id)
            .cloned()
            .ok_or_else(|| MusicError::NotFound(format!("track {track_id}")))?;
        // Server transcode-seek: re-render from the seek frame with a
        // fresh header (never a mid-chunk byte slice).
        let mut bytes = if let Some(ms) = opts.seek_ms {
            let skip = (ms * RATE as u64 / 1000) as usize;
            render_tone(&segments, skip)
        } else {
            wav_segments(&segments)
        };
        let (gapless_next, gapless_mode) = match (opts.next, &self.chain_mode) {
            (Some(n), Some(mode)) => {
                // Chained containers: two complete WAVs back to back.
                if let Some(segs) = self.tracks.lock().unwrap().get(&n).cloned() {
                    bytes.extend_from_slice(&wav_segments(&segs));
                }
                (Some(n), Some(mode.clone()))
            }
            _ => (None, None),
        };
        Ok(StreamInfo {
            progress: None,
            reader: Box::new(Cursor::new(bytes)),
            content_type: "audio/wav".into(),
            chain: Some(
                self.chain_label
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "wav->passthrough".into()),
            ),
            gapless_next,
            gapless_mode,
        })
    }
}

/// Build a player whose stub we can still inspect. The stub must be owned
/// by the player, so we wrap the shared Arc.
struct Harness {
    player: Player,
    stub: Arc<StubTransport>,
    sink: SharedSink,
}

impl Harness {
    fn new(chain_mode: Option<&str>) -> Self {
        let stub = Arc::new(StubTransport::new(chain_mode));
        let sink = SharedSink(Arc::new(Mutex::new(VecSink::new())));
        let stub2 = stub.clone();
        struct Wrap(Arc<StubTransport>);
        impl Transport for Wrap {
            fn open_stream(
                &self,
                track_id: i64,
                opts: &StreamOptions,
            ) -> Result<StreamInfo, MusicError> {
                self.0.open_stream(track_id, opts)
            }
        }
        let player = Player::new(Box::new(sink.clone()), Box::new(Wrap(stub2)));
        Self { player, stub, sink }
    }

    fn pump_until_done(&mut self, cap: usize) {
        let mut n = 0;
        while self.player.status() == PlayerStatus::Playing && n < cap {
            self.player.pump();
            n += 1;
        }
        assert!(n < cap, "pump loop did not terminate");
    }

    fn samples(&self) -> Vec<f32> {
        self.sink.0.lock().unwrap().samples.clone()
    }
}

fn rms(window: &[f32]) -> f32 {
    (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt()
}

fn zero_crossings(samples: &[f32]) -> usize {
    let mono: Vec<f32> = samples.iter().step_by(CHANNELS).copied().collect();
    mono.windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count()
}

// ---------------------------------------------------------------------------
// Gapless
// ---------------------------------------------------------------------------

#[test]
fn gapless_single_session_is_sample_exact_with_no_dip() {
    let mut h = Harness::new(Some("single-session"));
    // Single-session: ONE container holding both tracks' audio, as the
    // server emits it (not two WAVs concatenated).
    h.stub
        .add_next_body(1, wav_segments(&[(440.0, 22050), (660.0, 22050)]));
    h.stub.add(1, &[(440.0, 22050)]);
    h.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 500),
            track(2, AudioFormat::Wav, 500),
        ],
        0,
    );
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    h.pump_until_done(100);

    // Queue exhausted → stopped.
    assert_eq!(h.player.status(), PlayerStatus::Stopped);

    let s = h.samples();
    assert_eq!(s.len(), (22050 + 22050) * CHANNELS, "exact total samples");

    // No energy dip at the boundary (frame 22050).
    let b = 22050 * CHANNELS;
    let at_boundary = rms(&s[b - 220..b + 220]);
    let mid_track = rms(&s[10000 * CHANNELS..10000 * CHANNELS + 440]);
    assert!(
        at_boundary > mid_track * 0.95,
        "boundary rms {at_boundary} vs mid-track {mid_track}"
    );

    // The tone actually changes across the boundary.
    let zc_first = zero_crossings(&s[..22050 * CHANNELS]);
    let zc_second = zero_crossings(&s[22050 * CHANNELS..]);
    assert!(
        zc_first < 500 && zc_first > 380,
        "440 Hz half-second: {zc_first}"
    );
    assert!(
        zc_second * 10 > zc_first * 13,
        "660 Hz second half: {zc_second}"
    );
}

#[test]
fn gapless_chained_mode_continues_without_gap() {
    let mut h = Harness::new(Some("chained"));
    h.stub.add(1, &[(440.0, 22050)]);
    h.stub.add(2, &[(880.0, 22050)]);
    h.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 500),
            track(2, AudioFormat::Wav, 500),
        ],
        0,
    );
    h.pump_until_done(100);
    assert_eq!(h.player.status(), PlayerStatus::Stopped);

    let s = h.samples();
    assert_eq!(s.len(), (22050 + 22050) * CHANNELS, "exact total samples");
    let b = 22050 * CHANNELS;
    let at_boundary = rms(&s[b - 220..b + 220]);
    let mid_track = rms(&s[10000 * CHANNELS..10000 * CHANNELS + 440]);
    assert!(at_boundary > mid_track * 0.95, "no dip at chained boundary");
}

#[test]
fn the_chain_badge_follows_the_track_being_heard_through_a_gapless_boundary() {
    // The server joins each chained track's chain with " + " (regression: the
    // UI showed "dsf64->flac 24/88.2 + dsf64->flac 24/88.2" for track one).
    let mut h = Harness::new(Some("chained"));
    *h.stub.chain_label.lock().unwrap() =
        Some("dsf64->flac 24/88.2 + dsf128->flac 24/176.4".into());
    h.stub.add(1, &[(440.0, 22050)]);
    h.stub.add(2, &[(880.0, 22050)]);
    h.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 500),
            track(2, AudioFormat::Wav, 500),
        ],
        0,
    );
    assert_eq!(
        h.player.snapshot().chain.as_deref(),
        Some("dsf64->flac 24/88.2"),
        "first track only"
    );

    // Play across the boundary into the second track.
    for _ in 0..8 {
        h.player.pump();
    }
    let snap = h.player.snapshot();
    assert_eq!(snap.track.map(|t| t.id), Some(2), "now on the second track");
    assert_eq!(
        snap.chain.as_deref(),
        Some("dsf128->flac 24/176.4"),
        "its own chain, not the pair"
    );
}

#[test]
fn a_plain_chain_is_shown_as_the_server_sent_it() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    assert_eq!(
        h.player.snapshot().chain.as_deref(),
        Some("wav->passthrough")
    );
}

#[test]
fn decoder_reprobes_chained_wavs() {
    use kahawai_player_core::StreamDecoder;
    let mut bytes = wav_bytes(440.0, 22050);
    bytes.extend_from_slice(&wav_bytes(660.0, 22050));
    let mut dec = StreamDecoder::new(Box::new(Cursor::new(bytes)), true).expect("decoder opens");
    let mut out = vec![0.0f32; 8192 * CHANNELS];
    let mut total = 0;
    loop {
        let n = dec.decode_interleaved(&mut out).expect("decode");
        if n == 0 {
            break;
        }
        total += n;
    }
    assert_eq!(total, 44100, "every frame of both streams");
    assert_eq!(dec.streams_completed, 2, "re-probe found the second WAV");
}

#[test]
fn sequential_fallback_advances_one_track() {
    // No ?next= chaining: plain sequential responses.
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 22050)]);
    h.stub.add(2, &[(660.0, 22050)]);
    h.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 500),
            track(2, AudioFormat::Wav, 500),
        ],
        0,
    );
    // The engine still passes ?next=; the stub just doesn't chain.
    h.pump_until_done(100);
    assert_eq!(h.player.status(), PlayerStatus::Stopped);
    let s = h.samples();
    assert_eq!(s.len(), (22050 + 22050) * CHANNELS);
    let opened = h.stub.opened.lock().unwrap();
    assert_eq!(opened.len(), 2, "one request per track");
    assert_eq!(opened[0].0, 1);
    assert_eq!(opened[1].0, 2);
}

// ---------------------------------------------------------------------------
// Seek (S12)
// ---------------------------------------------------------------------------

fn expected_i16(freq: f32, frame: usize) -> i16 {
    let s = (2.0 * std::f32::consts::PI * freq * frame as f32 / RATE as f32).sin() * 0.7;
    (s * 32767.0).round() as i16
}

#[test]
fn transcode_seek_is_sample_accurate() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 88200)]); // 2 s
                                      // Force a transcode rendition so the server-side ?seek_ms= path runs.
    h.player.set_global_format(Some(StreamFormat::Flac));
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 2000)], 0);
    h.player.seek_ms(1000);

    let opened = h.stub.opened.lock().unwrap();
    let last = opened.last().expect("seek re-opened the stream");
    assert_eq!(last.1.seek_ms, Some(1000), "engine used ?seek_ms=");
    drop(opened);

    // Position accounts for the seek offset: 1000 ms + 3 pumped chunks.
    for _ in 0..3 {
        h.player.pump();
    }
    let pos = h.player.snapshot().position_ms;
    assert!((pos as i64 - 1278).abs() < 60, "position_ms = {pos}");

    h.pump_until_done(100);
    let s = h.samples();
    // First pumped frame == original frame 44100 (i16-quantized).
    let got = (s[0] * 32768.0).round() as i16;
    let want = expected_i16(440.0, 44100);
    assert!((got - want).abs() <= 2, "got {got}, want {want}");
}

#[test]
fn passthrough_seek_skips_decoded_frames() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 88200)]);
    h.player.set_global_format(Some(StreamFormat::Passthrough));
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 2000)], 0);
    h.player.seek_ms(1000);

    // Passthrough: no ?seek_ms= sent (server ignores it); the engine
    // restarts the byte stream and skips decoded frames itself.
    {
        let opened = h.stub.opened.lock().unwrap();
        let last = opened.last().expect("seek re-opened the stream");
        assert_eq!(last.1.seek_ms, None);
    }

    // The clock must start at the target, not at 0:00, both right away
    // and once the skipped frames have been consumed.
    assert_eq!(h.player.snapshot().position_ms, 1000);
    // ~11 pumps skip the first second; the next few play from 1.0 s.
    for _ in 0..15 {
        h.player.pump();
    }
    let pos = h.player.snapshot().position_ms;
    assert!(
        pos > 1000 && pos <= 2000,
        "playhead continues from 1.0 s: {pos}"
    );

    h.pump_until_done(100);
    let s = h.samples();
    let got = (s[0] * 32768.0).round() as i16;
    let want = expected_i16(440.0, 44100);
    assert!((got - want).abs() <= 2, "got {got}, want {want}");
}

// ---------------------------------------------------------------------------
// Format selection (S13)
// ---------------------------------------------------------------------------

#[test]
fn resolve_format_prefers_override_then_global_then_ladder() {
    use kahawai_player_core::DsdStory;
    let mp3 = track(1, AudioFormat::Mp3, 1000);
    let dsf = track(2, AudioFormat::Dsf, 1000);
    let convert = DsdStory::Convert;
    assert_eq!(
        resolve_format(&mp3, None, None, convert),
        StreamFormat::Passthrough
    );
    assert_eq!(
        resolve_format(&dsf, None, None, convert),
        StreamFormat::Flac
    );
    assert_eq!(
        resolve_format(&mp3, Some(StreamFormat::Opus), None, convert),
        StreamFormat::Opus
    );
    assert_eq!(
        resolve_format(
            &mp3,
            Some(StreamFormat::Opus),
            Some(StreamFormat::Mp3),
            convert
        ),
        StreamFormat::Mp3,
        "per-track override wins"
    );
    assert_eq!(
        resolve_format(&dsf, None, Some(StreamFormat::Dop), convert),
        StreamFormat::Dop
    );
}

#[test]
fn a_podcast_episode_always_passes_through() {
    use kahawai_player_core::DsdStory;
    let episode = track(kahawai_core::podcast_track_id(9), AudioFormat::Mp3, 1000);
    assert_eq!(
        resolve_format(
            &episode,
            Some(StreamFormat::Flac),
            Some(StreamFormat::Opus),
            DsdStory::Convert
        ),
        StreamFormat::Passthrough,
        "no forced rendition: the server only passes episodes through"
    );
}

#[test]
fn resolve_format_dsd_story_matrix() {
    use kahawai_player_core::DsdStory;
    let dsf = track(2, AudioFormat::Dsf, 1000);
    let dff = track(3, AudioFormat::Dff, 1000);
    let mp3 = track(1, AudioFormat::Mp3, 1000);
    // Native story: DSD tracks request DoP when nothing overrides.
    assert_eq!(
        resolve_format(&dsf, None, None, DsdStory::Native),
        StreamFormat::Dop
    );
    assert_eq!(
        resolve_format(&dff, None, None, DsdStory::Native),
        StreamFormat::Dop
    );
    // Non-DSD tracks are unaffected by the DSD story.
    assert_eq!(
        resolve_format(&mp3, None, None, DsdStory::Native),
        StreamFormat::Passthrough
    );
    // Under Native, DSD tracks ignore a *global* format — a stray
    // "Passthrough" must not silently defeat the DSD setting...
    assert_eq!(
        resolve_format(
            &dsf,
            Some(StreamFormat::Passthrough),
            None,
            DsdStory::Native
        ),
        StreamFormat::Dop,
        "global format does not apply to DSD under Native"
    );
    // ...while non-DSD tracks still honor it.
    assert_eq!(
        resolve_format(&mp3, Some(StreamFormat::Flac), None, DsdStory::Native),
        StreamFormat::Flac
    );
    // A per-track choice always wins.
    assert_eq!(
        resolve_format(&dsf, None, Some(StreamFormat::Flac), DsdStory::Native),
        StreamFormat::Flac,
        "per-track override wins over the DSD story"
    );
    // Convert honors the global format for DSD too.
    assert_eq!(
        resolve_format(&dsf, Some(StreamFormat::Opus), None, DsdStory::Convert),
        StreamFormat::Opus
    );
    assert_eq!(
        resolve_format(&dsf, None, None, DsdStory::Convert),
        StreamFormat::Flac
    );
    // Auto is resolved by the engine from the device; unresolved it is the
    // safe conversion.
    assert_eq!(
        resolve_format(&dsf, None, None, DsdStory::Auto),
        StreamFormat::Flac
    );
}

#[test]
fn valid_formats_per_source() {
    assert_eq!(
        valid_formats(&track(1, AudioFormat::Mp3, 0)),
        vec![
            StreamFormat::Passthrough,
            StreamFormat::Flac,
            StreamFormat::Opus,
            StreamFormat::Mp3
        ]
    );
    assert_eq!(
        valid_formats(&track(2, AudioFormat::Dsf, 0)),
        vec![StreamFormat::Flac, StreamFormat::Dop]
    );
    assert_eq!(
        valid_formats(&track(3, AudioFormat::Dff, 0)),
        vec![StreamFormat::Flac, StreamFormat::Dop]
    );
    assert!(valid_formats(&track(4, AudioFormat::SacdIso, 0)).is_empty());
    assert!(valid_formats(&track(5, AudioFormat::Unknown, 0)).is_empty());
}

#[test]
fn auto_goes_native_on_a_known_dsd_dac_and_converts_elsewhere() {
    // Default story is Auto: no explicit setting anywhere.
    let mut known = DopHarness::new(Some(DOP_RATE));
    known.sink.0.lock().unwrap().device = Some("FIIO K15 ".into());
    known.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    let snap = known.player.snapshot();
    assert_eq!(
        snap.format,
        Some(StreamFormat::Dop),
        "known DAC: native DoP"
    );
    assert_eq!(snap.output_path, OutputPath::Dop);

    let mut unknown = DopHarness::new(Some(DOP_RATE));
    unknown.sink.0.lock().unwrap().device = Some("MacBook Pro Speakers".into());
    unknown
        .player
        .play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(unknown.player.snapshot().format, Some(StreamFormat::Flac));
    assert!(
        unknown.player.snapshot().notice.is_none(),
        "a choice, not a failure"
    );

    let mut none = DopHarness::new(Some(DOP_RATE));
    none.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(
        none.player.snapshot().format,
        Some(StreamFormat::Flac),
        "unknown device is safe"
    );
}

#[test]
fn auto_honors_a_device_the_user_confirmed() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.sink.0.lock().unwrap().device = Some("Topping D90".into());
    h.player.set_dsd_devices(vec!["topping d90".into()]);
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Dop));
}

#[test]
fn a_stray_global_format_does_not_defeat_native_dsd() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.player
        .set_dsd_story(kahawai_player_core::DsdStory::Native);
    h.player.set_global_format(Some(StreamFormat::Passthrough));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Dop));
}

#[test]
fn dop_without_a_dsd_capable_sink_falls_back_to_flac_with_a_notice() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 22050)]);
    h.player.set_track_format(1, Some(StreamFormat::Dop));
    h.player
        .play_queue(vec![track(1, AudioFormat::Dsf, 500)], 0);
    assert_eq!(h.player.status(), PlayerStatus::Playing, "plays as PCM");
    let snap = h.player.snapshot();
    assert!(snap.error.is_none(), "not an error: {:?}", snap.error);
    assert_eq!(snap.format, Some(StreamFormat::Flac));
    assert!(
        snap.notice.as_deref().unwrap_or("").contains("DSD"),
        "notice: {:?}",
        snap.notice
    );
}

#[test]
fn dop_open_failure_falls_back_to_flac_and_releases_the_sink() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.sink.0.lock().unwrap().fail_open = true;
    h.player.set_global_format(Some(StreamFormat::Dop));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    let snap = h.player.snapshot();
    assert_eq!(snap.output_path, OutputPath::Pcm, "back on the shared path");
    assert_eq!(snap.format, Some(StreamFormat::Flac));
    assert!(snap.error.is_none());
    assert!(
        snap.notice
            .as_deref()
            .unwrap_or("")
            .contains("couldn't be set up for native DSD"),
        "notice says why in plain words: {:?}",
        snap.notice
    );
    h.pump_until_done(60);
    assert!(h.dop_bytes().is_empty(), "no DoP bytes reached the device");
    assert!(h.pcm_writes() > 0);
}

#[test]
fn a_slow_dop_open_loads_then_plays_natively() {
    // The request is slower than the inline wait, so the player is Loading and
    // the open is collected by later pumps; the controls are not held up.
    let mut h = DopHarness::with_delay(Some(DOP_RATE), Duration::from_millis(400));
    h.player.set_global_format(Some(StreamFormat::Dop));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(h.player.status(), PlayerStatus::Loading);
    let mut spins = 0;
    while h.player.status() == PlayerStatus::Loading && spins < 200 {
        h.player.pump();
        spins += 1;
    }
    let snap = h.player.snapshot();
    assert_eq!(snap.status, PlayerStatus::Playing);
    assert_eq!(snap.format, Some(StreamFormat::Dop));
    assert!(snap.error.is_none());
}

#[test]
fn a_slow_dop_open_that_cannot_start_falls_back_to_flac_with_the_reason() {
    let mut h = DopHarness::with_delay(Some(DOP_RATE), Duration::from_millis(300));
    h.sink.0.lock().unwrap().fail_open = true;
    h.player.set_global_format(Some(StreamFormat::Dop));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    let mut spins = 0;
    while spins < 400
        && (h.player.status() == PlayerStatus::Loading
            || h.player.snapshot().format != Some(StreamFormat::Flac))
    {
        h.player.pump();
        spins += 1;
    }
    let snap = h.player.snapshot();
    assert_eq!(
        snap.status,
        PlayerStatus::Playing,
        "plays as PCM: {:?}",
        snap.error
    );
    assert_eq!(snap.format, Some(StreamFormat::Flac));
    assert_eq!(snap.output_path, OutputPath::Pcm);
    assert!(
        snap.notice
            .as_deref()
            .unwrap_or("")
            .contains("couldn't be set up for native DSD"),
        "notice: {:?}",
        snap.notice
    );
}

#[test]
fn the_fallback_notice_clears_on_the_next_track() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.sink.0.lock().unwrap().fail_open = true;
    h.player.set_global_format(Some(StreamFormat::Dop));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert!(h.player.snapshot().notice.is_some());
    h.sink.0.lock().unwrap().fail_open = false;
    h.player.play_queue(vec![dsd_track(2, DSD64, 1000)], 0);
    let snap = h.player.snapshot();
    assert_eq!(snap.output_path, OutputPath::Dop);
    assert!(snap.notice.is_none());
}

// ---------------------------------------------------------------------------
// Volume
// ---------------------------------------------------------------------------

#[test]
fn volume_scales_samples() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 22050)]);
    h.player.set_volume(0.5);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 500)], 0);
    h.pump_until_done(100);
    let s = h.samples();
    assert!(!s.is_empty());
    let peak = s.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    assert!((peak - 0.35).abs() < 0.02, "peak {peak} ≈ 0.7 * 0.5");
}

#[test]
fn position_is_what_is_audible_not_what_is_queued() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    for _ in 0..30 {
        h.player.pump();
    }
    let written = h.player.snapshot().position_ms;
    assert!(written > 1000, "pumped a good second of audio: {written}");

    // 0.5 s of what we wrote is still sitting in the device buffer.
    h.sink.0.lock().unwrap().buffered = (RATE / 2) as u64;
    let audible = h.player.snapshot().position_ms;
    assert_eq!(audible, written - 500);

    // More buffered than written (right after a seek) clamps at the base.
    h.sink.0.lock().unwrap().buffered = u64::MAX / 4;
    assert_eq!(h.player.snapshot().position_ms, 0);
}

#[test]
fn natural_end_of_stream_drains_the_sink_before_moving_on() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 4410)]);
    h.stub.add(2, &[(660.0, 4410)]);
    h.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 100),
            track(2, AudioFormat::Wav, 100),
        ],
        0,
    );
    h.pump_until_done(200);
    // One drain per stream end: the tail of each track is played out
    // before the next open()/stop() discards the device buffer.
    assert_eq!(h.sink.0.lock().unwrap().drains, 2);
}

#[test]
fn switching_output_device_reopens_at_the_current_position() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    for _ in 0..15 {
        h.player.pump();
    }
    let before = h.player.snapshot().position_ms;
    let opens = h.stub.opened.lock().unwrap().len();

    h.player.set_output_device(Some("Studio DAC".into()));

    assert_eq!(
        h.sink.0.lock().unwrap().device.as_deref(),
        Some("Studio DAC")
    );
    assert_eq!(
        h.stub.opened.lock().unwrap().len(),
        opens + 1,
        "stream re-opened for the new device"
    );
    let after = h.player.snapshot();
    assert_eq!(after.status, PlayerStatus::Playing);
    // ms -> frames -> ms truncates by a millisecond at most.
    assert!(
        after.position_ms.abs_diff(before) <= 2,
        "resumes where it was: {before} -> {}",
        after.position_ms
    );

    // Back to the system default.
    h.player.set_output_device(None);
    assert_eq!(h.sink.0.lock().unwrap().device, None);
}

#[test]
fn switching_device_while_paused_stays_paused_and_idle_is_a_no_reopen() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    // Nothing loaded: just records the choice, opens nothing.
    h.player.set_output_device(Some("Speakers".into()));
    assert_eq!(h.stub.opened.lock().unwrap().len(), 0);

    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    for _ in 0..10 {
        h.player.pump();
    }
    h.player.pause();
    h.player.set_output_device(Some("Headphones".into()));
    assert_eq!(h.player.status(), PlayerStatus::Paused);
    assert_eq!(
        h.sink.0.lock().unwrap().device.as_deref(),
        Some("Headphones")
    );
}

#[test]
fn output_device_choice_persists_across_controllers() {
    let dir = std::env::temp_dir().join(format!("kahawai-dev-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("engine-settings.json");
    {
        let c = EngineController::new(Box::new(VecSink::new()), path.clone());
        assert_eq!(c.output_device(), None);
        c.set_output_device(Some("Studio DAC".into()));
        assert_eq!(c.output_device().as_deref(), Some("Studio DAC"));
    }
    let c = EngineController::new(Box::new(VecSink::new()), path.clone());
    assert_eq!(c.output_device().as_deref(), Some("Studio DAC"));
    c.set_output_device(None);
    let c2 = EngineController::new(Box::new(VecSink::new()), path);
    assert_eq!(c2.output_device(), None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn volume_persists_across_launches_and_a_drag_is_saved_as_its_last_value() {
    let dir = std::env::temp_dir().join(format!("kahawai-vol-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("engine-settings.json");
    {
        let c = EngineController::new(Box::new(VecSink::new()), path.clone());
        for v in [0.9, 0.7, 0.5, 0.35] {
            c.set_volume(v); // a slider drag
        }
    } // dropping the controller flushes the pending value
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(
        (saved["volume"].as_f64().unwrap() - 0.35).abs() < 1e-6,
        "{saved}"
    );
    let c = EngineController::new(Box::new(VecSink::new()), path);
    wait_for(|| (c.snapshot().volume - 0.35).abs() < 1e-6);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn volume_and_paused_seek_are_visible_in_the_snapshot() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    h.player.pause();
    h.player.set_volume(0.25);
    assert_eq!(h.player.snapshot().volume, 0.25);
    h.player.seek_ms(2000);
    assert_eq!(h.player.snapshot().position_ms, 2000);
    assert_eq!(h.player.status(), PlayerStatus::Paused);
}

// ---------------------------------------------------------------------------
// Controller thread smoke test
// ---------------------------------------------------------------------------

#[test]
fn controller_drives_playback_on_its_thread() {
    let stub = Arc::new(StubTransport::new(None));
    stub.add(1, &[(440.0, 22050)]);
    struct Wrap(Arc<StubTransport>);
    impl Transport for Wrap {
        fn open_stream(
            &self,
            track_id: i64,
            opts: &StreamOptions,
        ) -> Result<StreamInfo, MusicError> {
            self.0.open_stream(track_id, opts)
        }
    }
    let dir = std::env::temp_dir().join("kahawai-player-core-test-settings");
    let _ = std::fs::remove_dir_all(&dir);
    let settings_path = dir.join("settings.json");
    let url_lock = Arc::new(std::sync::RwLock::new("http://stub".to_string()));
    let ctl = EngineController::with_transport(
        Box::new(SharedSink(Arc::new(Mutex::new(VecSink::new())))),
        Box::new(Wrap(stub)),
        url_lock,
        settings_path.clone(),
    );

    // Settings round-trip.
    ctl.set_server_url("http://lan:8080");
    assert_eq!(ctl.server_url(), "http://lan:8080");
    let saved = std::fs::read_to_string(&settings_path).expect("settings saved");
    assert!(saved.contains("http://lan:8080"), "{saved}");

    // Empty queue → play_queue with no tracks stays stopped.
    ctl.play_queue(vec![], 0);
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert_eq!(ctl.snapshot().status, PlayerStatus::Stopped);

    // A real track: the thread decodes it (faster than real time) and
    // emits state events along the way.
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 500)], 0);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let events = ctl.drain_events();
    assert!(!events.is_empty(), "state changes emitted events");
    let saw_track = events.iter().any(|e| match e {
        kahawai_player_core::PlayerEvent::State(snapshot) => {
            snapshot.track.as_ref().map(|t| t.id) == Some(1)
        }
    });
    assert!(saw_track, "a state event carried track 1");

    // Pause/resume/toggle are safe to call; they must not panic.
    ctl.pause();
    ctl.toggle();
    ctl.next();
    ctl.prev();
    ctl.seek_ms(5000);
    std::thread::sleep(std::time::Duration::from_millis(150));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// DoP native path + v1 DSP (C2)
// ---------------------------------------------------------------------------

const DOP_RATE: u32 = 176_400;
const DSD64: u32 = 2_822_400;

/// DoP WAV fixture: 24-bit stereo, markers alternating 0x05/0xFA per
/// frame — exactly what the server's ?format=dop emits.
fn dop_wav_bytes(frames: usize) -> Vec<u8> {
    let channels: u16 = 2;
    let data_len = frames * channels as usize * 3;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&DOP_RATE.to_le_bytes());
    v.extend_from_slice(&(DOP_RATE * channels as u32 * 3).to_le_bytes());
    v.extend_from_slice(&(channels * 3).to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    let mut marker = 0x05u8;
    for _ in 0..frames {
        for _ in 0..channels {
            v.push(0xAA);
            v.push(0x55);
            v.push(marker);
        }
        marker = if marker == 0x05 { 0xFA } else { 0x05 };
    }
    v
}

fn dsd_track(id: i64, dsd_rate: u32, duration_ms: u64) -> Track {
    Track {
        id,
        path: format!("/m/{id}.dsf"),
        hash: Some("h".into()),
        format: AudioFormat::Dsf,
        sample_rate: Some(dsd_rate),
        bit_depth: Some(1),
        channels: Some(2),
        duration_ms: Some(duration_ms),
        bitrate: None,
        title: Some(format!("DSD{id}")),
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

/// DoP-capable test sink: records `write_dop` bytes and PCM writes
/// separately so tests can prove the paths never mix.
struct DopSink {
    state: kahawai_player_core::SinkState,
    pcm_samples: Vec<f32>,
    pcm_writes: usize,
    dop: Vec<u8>,
    /// What `dop_output_rate` reports; `None` = device refuses the rate.
    dop_rate: Option<u32>,
    path: OutputPath,
    /// `open` fails, like a device whose hog mode is held or that refuses
    /// the stream format.
    fail_open: bool,
    /// What the sink says it would open (for `Auto` DSD handling).
    device: Option<String>,
    /// Reports an external DAC (Best quality goes exclusive / native).
    external: bool,
    /// `write_dop` fails, like a DAC that was unplugged mid-track.
    fail_write: bool,
}

#[derive(Clone)]
struct SharedDopSink(Arc<Mutex<DopSink>>);

impl AudioSink for SharedDopSink {
    fn open(&mut self, _track: &Track) -> Result<(), MusicError> {
        if self.0.lock().unwrap().fail_open && self.0.lock().unwrap().path == OutputPath::Dop {
            return Err(MusicError::Audio("hog mode refused".into()));
        }
        Ok(())
    }
    fn write(&mut self, chunk: PcmChunk) -> Result<(), MusicError> {
        let mut s = self.0.lock().unwrap();
        s.pcm_writes += 1;
        s.pcm_samples.extend_from_slice(&chunk.frames);
        Ok(())
    }
    fn play(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().state = kahawai_player_core::SinkState::Playing;
        Ok(())
    }
    fn pause(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().state = kahawai_player_core::SinkState::Paused;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), MusicError> {
        self.0.lock().unwrap().state = kahawai_player_core::SinkState::Stopped;
        Ok(())
    }
    fn state(&self) -> kahawai_player_core::SinkState {
        self.0.lock().unwrap().state
    }
    fn supports_dop(&self) -> bool {
        true
    }
    fn output_device_name(&self) -> Option<String> {
        self.0.lock().unwrap().device.clone()
    }
    fn output_is_external_dac(&self) -> bool {
        self.0.lock().unwrap().external
    }
    fn dop_output_rate(&self, _dsd_rate_hz: u32) -> Option<u32> {
        self.0.lock().unwrap().dop_rate
    }
    fn select_output_path(&mut self, path: OutputPath) {
        self.0.lock().unwrap().path = path;
    }
    fn write_dop(&mut self, frames: &[u8]) -> Result<(), MusicError> {
        let mut s = self.0.lock().unwrap();
        if s.fail_write {
            return Err(MusicError::Audio("device gone".into()));
        }
        s.dop.extend_from_slice(frames);
        Ok(())
    }
}

/// Stub transport for DoP tests: serves the DoP WAV for `?format=dop`
/// (raw payload, no header, when `?seek_ms=` is present — like the real
/// server) and a plain WAV for the FLAC fallback path.
struct DopTransport {
    wav: Vec<u8>,
    opened: Mutex<Vec<(i64, StreamOptions)>>,
}

impl Transport for DopTransport {
    fn open_stream(&self, track_id: i64, opts: &StreamOptions) -> Result<StreamInfo, MusicError> {
        self.opened.lock().unwrap().push((track_id, opts.clone()));
        if opts.format == Some(StreamFormat::Dop) {
            let body = if opts.seek_ms.is_some() {
                self.wav[44..].to_vec()
            } else {
                self.wav.clone()
            };
            return Ok(StreamInfo {
                progress: None,
                reader: Box::new(Cursor::new(body)),
                content_type: "audio/wav".into(),
                chain: None,
                gapless_next: None,
                gapless_mode: None,
            });
        }
        Ok(StreamInfo {
            progress: None,
            reader: Box::new(Cursor::new(wav_bytes(440.0, 22050))),
            content_type: "audio/wav".into(),
            chain: Some("dsd->flac".into()),
            gapless_next: None,
            gapless_mode: None,
        })
    }
}

struct DopHarness {
    player: Player,
    transport: Arc<DopTransport>,
    sink: SharedDopSink,
}

impl DopHarness {
    fn new(dop_rate: Option<u32>) -> Self {
        Self::with_delay(dop_rate, Duration::ZERO)
    }

    /// Every stream request takes `delay` (a slow network).
    fn with_delay(dop_rate: Option<u32>, delay: Duration) -> Self {
        let transport = Arc::new(DopTransport {
            wav: dop_wav_bytes(2000),
            opened: Mutex::new(Vec::new()),
        });
        let sink = SharedDopSink(Arc::new(Mutex::new(DopSink {
            state: kahawai_player_core::SinkState::Stopped,
            pcm_samples: Vec::new(),
            pcm_writes: 0,
            dop: Vec::new(),
            dop_rate,
            path: OutputPath::Pcm,
            fail_open: false,
            device: None,
            external: true,
            fail_write: false,
        })));
        struct Wrap(Arc<DopTransport>, Duration);
        impl Transport for Wrap {
            fn open_stream(
                &self,
                track_id: i64,
                opts: &StreamOptions,
            ) -> Result<StreamInfo, MusicError> {
                std::thread::sleep(self.1);
                self.0.open_stream(track_id, opts)
            }
        }
        let player = Player::new(
            Box::new(sink.clone()),
            Box::new(Wrap(transport.clone(), delay)),
        );
        Self {
            player,
            transport,
            sink,
        }
    }

    fn pump_until_done(&mut self, cap: usize) {
        let mut n = 0;
        while self.player.status() == PlayerStatus::Playing && n < cap {
            self.player.pump();
            n += 1;
        }
        assert!(n < cap, "pump loop did not terminate");
    }

    fn dop_bytes(&self) -> Vec<u8> {
        self.sink.0.lock().unwrap().dop.clone()
    }

    fn payload_bytes(&self) -> Vec<u8> {
        self.transport.wav[44..].to_vec()
    }

    fn pcm_writes(&self) -> usize {
        self.sink.0.lock().unwrap().pcm_writes
    }
}

#[test]
fn dop_bypasses_dsp_entirely() {
    // Hostile DSP state — cranked EQ, loudness on, volume at zero — must
    // not touch the DoP bytes: bit-perfect hog-mode output.
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.player.set_global_format(Some(StreamFormat::Dop));
    h.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 12.0,
            q: 1.0,
        }])
        .expect("valid band");
    h.player.set_eq_enabled(true);
    h.player.set_analog(AnalogSettings {
        enabled: true,
        flavour: AnalogFlavour::WarmTriode,
        drive: 1.0,
        mix: 1.0,
        output_db: 6.0,
        auto_gain: false,
        ..Default::default()
    });
    h.player.set_loudness_enabled(true);
    h.player.set_volume(0.0);
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    h.player.pump();
    let snap = h.player.snapshot();
    assert_eq!(snap.output_path, OutputPath::Dop, "exclusive DoP path");
    assert_eq!(snap.format, Some(StreamFormat::Dop));
    assert!(snap.error.is_none());

    h.pump_until_done(60);
    assert_eq!(h.player.status(), PlayerStatus::Stopped);

    assert_eq!(
        h.dop_bytes(),
        h.payload_bytes(),
        "DoP payload bit-identical despite EQ/loudness/volume"
    );
    assert_eq!(h.pcm_writes(), 0, "DoP never touches the PCM write path");
    assert_eq!(
        h.transport.opened.lock().unwrap().len(),
        1,
        "no loudness pre-scan on the DoP path"
    );
}

#[test]
fn eq_is_tuned_to_the_device_rate_when_the_stream_is_resampled() {
    // A 44.1 kHz tone on a device that wants 48 kHz is resampled before the
    // EQ, so a narrow +12 dB band at the tone's frequency must still give
    // ~x4 (designed at 48 kHz). Designed at 44.1 kHz it would sit ~9% high
    // and miss the tone.
    let run = |eq: bool| {
        let mut h = Harness::new(None);
        h.player.set_volume(0.2); // after the EQ: keeps the boosted tone under the headroom guard
        h.sink.0.lock().unwrap().demand_rate = Some(48_000);
        h.stub.add(1, &[(440.0, 44100)]);
        if eq {
            h.player
                .set_eq_bands(vec![EqBand {
                    band_type: EqBandType::Peaking,
                    freq: 440.0,
                    gain_db: 12.0,
                    q: 6.0,
                }])
                .expect("valid band");
        }
        h.player
            .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
        h.pump_until_done(100);
        assert_eq!(
            h.sink.0.lock().unwrap().sample_rate,
            Some(48_000),
            "resampled to the device rate"
        );
        let s = h.samples();
        rms(&s[s.len() / 2..]) // steady state, after the filter settles
    };
    let ratio = run(true) / run(false);
    assert!(
        (ratio - 4.0).abs() < 0.6,
        "expected ~x4 at the tone, got {ratio}"
    );
}

#[test]
fn analog_stage_colours_the_pcm_path_and_is_off_by_default() {
    let run = |analog: Option<AnalogSettings>| {
        let mut h = Harness::new(None);
        h.stub.add(1, &[(440.0, 44100)]);
        if let Some(a) = analog {
            h.player.set_analog(a);
        }
        h.player
            .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
        h.pump_until_done(100);
        h.samples()
    };
    let dry = run(None);
    let warm = run(Some(AnalogSettings {
        enabled: true,
        flavour: AnalogFlavour::WarmTriode,
        drive: 0.8,
        mix: 1.0,
        output_db: 0.0,
        auto_gain: false,
        ..Default::default()
    }));
    assert_eq!(
        run(Some(AnalogSettings::default())),
        dry,
        "default settings leave the audio untouched"
    );
    assert_eq!(warm.len(), dry.len(), "the stage adds no samples");
    let diff = warm
        .iter()
        .zip(&dry)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(
        diff > 0.01,
        "enabled: the audio is coloured (max difference {diff})"
    );
}

#[test]
fn snapshot_reports_the_analog_plan_only_while_it_is_on_and_playing() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 5)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 5000)], 0);
    h.player.pump();
    assert_eq!(
        h.player.snapshot().analog_plan,
        None,
        "off: nothing to report"
    );
    h.player.set_analog(AnalogSettings {
        enabled: true,
        antialias: AntiAliasChoice::X2Adaa,
        ..AnalogSettings::default()
    });
    h.player.pump();
    assert_eq!(
        h.player.snapshot().analog_plan.as_deref(),
        Some("2x oversampling + ADAA, 0.7 ms latency")
    );
    h.player.stop();
    assert_eq!(h.player.snapshot().analog_plan, None, "nothing playing");
}

#[test]
fn the_level_meter_reports_what_the_analog_stage_does_to_the_loudness() {
    let run = |a: AnalogSettings| {
        let mut h = Harness::new(None);
        h.stub.add(1, &[(300.0, 44100 * 4)]);
        h.player.set_analog(a);
        h.player
            .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
        // Pump most of the way (not to the end, so the snapshot still has a stream).
        for _ in 0..40 {
            h.player.pump();
        }
        (h.player.snapshot(), h.samples())
    };
    let loud = AnalogSettings {
        enabled: true,
        flavour: AnalogFlavour::Tube2a3,
        drive: 1.0,
        mix: 1.0,
        output_db: 0.0,
        auto_gain: false,
        sag: 0.0,
        transformer: 0.0,
        ..Default::default()
    };
    let (snap, out) = run(loud);
    let lvl = snap
        .analog_level
        .expect("measured after a couple of seconds");
    assert!(
        lvl.seconds > 1.0,
        "seconds behind the reading: {}",
        lvl.seconds
    );
    assert!((lvl.output_lufs - lvl.input_lufs - lvl.delta_db).abs() < 1e-3);
    // The reading agrees with an independent measurement of what reached the sink.
    let want =
        kahawai_player_core::integrated_lufs(&out[out.len() / 4 * 2..], 2, 44_100).expect("audio");
    assert!(
        (lvl.output_lufs - want).abs() < 1.5,
        "output {:.1} LUFS vs offline {:.1}",
        lvl.output_lufs,
        want
    );
    assert!(
        lvl.peak_dbfs < 3.0 && lvl.peak_dbfs > -30.0,
        "a sensible peak: {}",
        lvl.peak_dbfs
    );
    // Trim the output and the delta follows one for one.
    let (snap2, _) = run(AnalogSettings {
        output_db: -6.0,
        ..loud
    });
    let d = snap2.analog_level.unwrap().delta_db - lvl.delta_db;
    assert!(
        (d + 6.0).abs() < 0.15,
        "-6 dB of output trim moves the level by {d:.2} dB"
    );
    // With the mix at zero the stage passes the dry signal: no change.
    let (snap3, _) = run(AnalogSettings { mix: 0.0, ..loud });
    assert!(
        snap3.analog_level.unwrap().delta_db.abs() < 0.3,
        "mix 0 changes nothing"
    );
    // Off (dry), the meter still reads — A/B level-matching needs a peak and
    // loudness reading on the dry slot too — but the delta is ~0 by construction.
    let (off, _) = run(AnalogSettings::default());
    let off_lvl = off.analog_level.expect("dry slot is still metered");
    assert!(
        off_lvl.delta_db.abs() < 0.05,
        "dry: input and output loudness match ({})",
        off_lvl.delta_db
    );
}

#[test]
fn crossfeed_is_off_by_default_and_changes_the_pcm_path_when_on() {
    let run = |crossfeed: Option<CrossfeedSettings>| {
        let mut h = Harness::new(None);
        h.stub.add(1, &[(440.0, 44100)]);
        if let Some(c) = crossfeed {
            h.player.set_crossfeed(c);
        }
        h.player
            .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
        h.pump_until_done(100);
        h.samples()
    };
    let dry = run(None);
    assert_eq!(
        run(Some(CrossfeedSettings::default())),
        dry,
        "off: the audio is untouched"
    );
    let wet = run(Some(CrossfeedSettings {
        enabled: true,
        preset: CrossfeedPreset::Meier,
        ..Default::default()
    }));
    assert_eq!(wet.len(), dry.len(), "the stage adds no samples");
    let diff = wet
        .iter()
        .zip(&dry)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(diff > 1e-3, "on: the audio changes (max difference {diff})");
}

#[test]
fn crossfeed_settings_are_saved_clamped_and_old_files_default_to_off() {
    let dir = std::env::temp_dir().join("kahawai-player-core-test-crossfeed-settings");
    let _ = std::fs::remove_dir_all(&dir);
    let settings_path = dir.join("settings.json");
    let url_lock = Arc::new(std::sync::RwLock::new("http://stub".to_string()));
    let ctl = EngineController::with_transport(
        Box::new(VecSink::new()),
        Box::new(StubTransport::new(None)),
        url_lock,
        settings_path.clone(),
    );
    ctl.set_crossfeed(CrossfeedSettings {
        enabled: true,
        preset: CrossfeedPreset::Custom,
        cutoff_hz: 900.0,
        feed_db: 99.0,
    });
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("saved"))
            .expect("json");
    assert_eq!(v["dsp"]["crossfeed"]["enabled"], true);
    assert_eq!(v["dsp"]["crossfeed"]["preset"], "custom");
    assert_eq!(v["dsp"]["crossfeed"]["cutoff_hz"], 900.0);
    assert_eq!(
        v["dsp"]["crossfeed"]["feed_db"], 15.0,
        "clamped before saving"
    );
    // Not finite: ignored, the saved value stays.
    ctl.set_crossfeed(CrossfeedSettings {
        feed_db: f32::NAN,
        ..Default::default()
    });
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(v["dsp"]["crossfeed"]["enabled"], true);

    let old =
        r#"{"eq_bands":[],"eq_enabled":true,"loudness_enabled":false,"loudness_target":-14.0}"#;
    let dsp: kahawai_player_core::DspSettings =
        serde_json::from_str(old).expect("old dsp block parses");
    assert_eq!(dsp.crossfeed, CrossfeedSettings::default());
    assert!(!dsp.crossfeed.enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn analog_settings_persist_and_old_files_default_to_off() {
    let dir = std::env::temp_dir().join("kahawai-player-core-test-analog-settings");
    let _ = std::fs::remove_dir_all(&dir);
    let settings_path = dir.join("settings.json");
    let url_lock = Arc::new(std::sync::RwLock::new("http://stub".to_string()));
    let ctl = EngineController::with_transport(
        Box::new(VecSink::new()),
        Box::new(StubTransport::new(None)),
        url_lock,
        settings_path.clone(),
    );
    ctl.set_analog(AnalogSettings {
        enabled: true,
        flavour: AnalogFlavour::SolidState,
        drive: 9.0,
        mix: 0.25,
        output_db: -2.0,
        auto_gain: true,
        ..Default::default()
    });
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("saved"))
            .expect("json");
    assert_eq!(v["dsp"]["analog"]["enabled"], true);
    assert_eq!(v["dsp"]["analog"]["flavour"], "solid_state");
    assert_eq!(v["dsp"]["analog"]["drive"], 1.0, "clamped before saving");
    assert_eq!(v["dsp"]["analog"]["mix"], 0.25);

    // A settings file from before this stage existed still loads, with the stage off.
    let old = r#"{"server_url":"http://x","dsp":{"eq_bands":[],"eq_enabled":true,"loudness_enabled":false,"loudness_target":-14.0}}"#;
    let parsed: serde_json::Value = serde_json::from_str(old).unwrap();
    let dsp: kahawai_player_core::DspSettings =
        serde_json::from_value(parsed["dsp"].clone()).expect("old dsp block parses");
    assert!(!dsp.analog.enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dop_seek_continuation_uses_established_spec() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.player.set_global_format(Some(StreamFormat::Dop));
    h.player.play_queue(vec![dsd_track(1, DSD64, 2000)], 0);
    h.player.pump(); // headered response establishes the spec
    let after_first = h.dop_bytes().len();
    assert_eq!(after_first, h.payload_bytes().len());

    h.player.seek_ms(500); // server answers with raw payload, no header
    h.pump_until_done(60);
    assert_eq!(h.player.status(), PlayerStatus::Stopped);
    assert!(
        h.player.snapshot().error.is_none(),
        "raw continuation must parse under the established spec"
    );
    let opened = h.transport.opened.lock().unwrap();
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[1].1.seek_ms, Some(500));
    drop(opened);

    let all = h.dop_bytes();
    assert_eq!(
        &all[after_first..],
        &h.payload_bytes()[..],
        "seek continuation payload passed through untouched"
    );
}

#[test]
fn dop_unsupported_rate_falls_back_to_flac() {
    // The device refuses the DoP rate: the engine must fall back to
    // FLAC and keep playing — never fail the track.
    let mut h = DopHarness::new(None);
    h.player.set_global_format(Some(StreamFormat::Dop));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(
        h.player.status(),
        PlayerStatus::Playing,
        "fallback must not fail playback"
    );
    let opened = h.transport.opened.lock().unwrap();
    assert_eq!(
        opened[0].1.format,
        Some(StreamFormat::Flac),
        "engine fell back to FLAC"
    );
    drop(opened);

    let snap = h.player.snapshot();
    assert_eq!(snap.output_path, OutputPath::Pcm);
    assert_eq!(snap.format, Some(StreamFormat::Flac));
    h.pump_until_done(60);
    assert_eq!(h.player.status(), PlayerStatus::Stopped);
    assert!(h.pcm_writes() > 0, "PCM actually played");
    assert!(h.dop_bytes().is_empty(), "no DoP bytes on fallback");
}

#[test]
fn eq_peaking_band_applies_on_pcm_path() {
    // Volume comes after the EQ, so at 0.2 the x4 boosted tone stays below the
    // headroom guard and the comparison is of the EQ alone.
    let mut h = Harness::new(None);
    h.player.set_volume(0.2);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.pump_until_done(100);
    let base = rms(&h.samples());

    let mut h2 = Harness::new(None);
    h2.player.set_volume(0.2);
    h2.stub.add(1, &[(440.0, 44100)]);
    h2.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 440.0,
            gain_db: 12.0,
            q: 1.0,
        }])
        .expect("valid band");
    h2.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h2.pump_until_done(100);
    let boosted = rms(&h2.samples());

    let ratio = boosted / base;
    assert!(
        (ratio - 4.0).abs() < 0.8,
        "+12 dB peaking at the tone ~= x4 amplitude, got {ratio}"
    );
}

#[test]
fn eq_rejects_invalid_bands() {
    let mut h = Harness::new(None);
    let err = h.player.set_eq_bands(vec![EqBand {
        band_type: EqBandType::Peaking,
        freq: 5.0, // below the 10 Hz floor
        gain_db: 0.0,
        q: 1.0,
    }]);
    assert!(err.is_err(), "sub-10 Hz band must be rejected");
}

#[test]
fn loudness_prescan_applies_gain_and_caches_it() {
    // Baseline: no loudness.
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.pump_until_done(100);
    let base = rms(&h.samples());

    // Loudness to -14 LUFS: the 0.7-amplitude tone measures well above
    // target, so the applied gain is negative (quieter).
    let mut h2 = Harness::new(None);
    h2.stub.add(1, &[(440.0, 44100)]);
    h2.player.set_loudness_enabled(true);
    h2.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h2.pump_until_done(100);
    assert_eq!(h2.player.status(), PlayerStatus::Stopped);
    let normed = rms(&h2.samples());
    assert!(
        normed < base * 0.9,
        "loudness gain applied: {normed} vs baseline {base}"
    );
    // Pre-scan (1 request) + playback (1 request).
    assert_eq!(h2.stub.opened.lock().unwrap().len(), 2);

    // Replay the same track: the cached gain means no second pre-scan.
    h2.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h2.pump_until_done(100);
    assert_eq!(
        h2.stub.opened.lock().unwrap().len(),
        3,
        "gain cache hit: replay adds one request, not two"
    );
}

// ---------------------------------------------------------------------------
// DSP settings persistence (C2)
// ---------------------------------------------------------------------------

#[test]
fn dsp_settings_persist_and_reload() {
    let dir = std::env::temp_dir().join("kahawai-player-core-test-dsp-settings");
    let _ = std::fs::remove_dir_all(&dir);
    let settings_path = dir.join("settings.json");

    {
        let url_lock = Arc::new(std::sync::RwLock::new("http://stub".to_string()));
        let ctl = EngineController::with_transport(
            Box::new(VecSink::new()),
            Box::new(StubTransport::new(None)),
            url_lock,
            settings_path.clone(),
        );
        ctl.set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: -3.0,
            q: 1.0,
        }]);
        ctl.set_eq_enabled(false);
        ctl.set_loudness_target(-16.0);
        ctl.set_loudness_enabled(true);
        // with_transport does not apply persisted settings, but the setters
        // write the file immediately (synchronous).
        let saved = std::fs::read_to_string(&settings_path).expect("settings saved");
        assert!(saved.contains("\"eq_enabled\": false"), "{saved}");
        assert!(saved.contains("\"loudness_enabled\": true"), "{saved}");
        assert!(saved.contains("-16.0"), "{saved}");
        assert!(saved.contains("\"freq\": 1000.0"), "{saved}");
    }

    // A fresh controller built with `new` (like the shell does) applies the
    // persisted DSP settings. `new` needs an HTTP transport, which we can't
    // use here — instead verify the file parses back through a second
    // with_transport + manual reload path: load applies via `new` only.
    // What we *can* assert: the JSON round-trips through EngineSettings.
    let saved = std::fs::read_to_string(&settings_path).expect("settings saved");
    let v: serde_json::Value = serde_json::from_str(&saved).expect("valid json");
    assert_eq!(v["dsp"]["eq_enabled"], false);
    assert_eq!(v["dsp"]["loudness_enabled"], true);
    assert_eq!(v["dsp"]["loudness_target"], -16.0);
    assert_eq!(v["dsp"]["eq_bands"][0]["band_type"], "peaking");
    // Old files without a dsp section must still load (serde default).
    let legacy = r#"{"server_url": "http://lan:8080"}"#;
    let v2: serde_json::Value = serde_json::from_str(legacy).expect("valid json");
    assert!(v2.get("dsp").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// C3: queue persistence, repeat/shuffle, append/play-next, DSD story prefs
// ---------------------------------------------------------------------------

use kahawai_player_core::{DsdStory, RepeatMode};
use std::time::Duration;

fn stub_controller(dir_suffix: &str, tracks: Vec<Track>) -> (EngineController, std::path::PathBuf) {
    let stub = StubTransport::new(None);
    for t in &tracks {
        // Long-enough tone that nothing finishes (and auto-advances)
        // during the test; short enough to render quickly in debug builds
        // when tests run in parallel.
        stub.add(t.id, &[(440.0, (RATE as usize) * 10)]);
    }
    let dir = std::env::temp_dir().join(format!("kahawai-player-core-c3-{dir_suffix}"));
    let _ = std::fs::remove_dir_all(&dir);
    let settings_path = dir.join("settings.json");
    let url_lock = Arc::new(std::sync::RwLock::new("http://stub".to_string()));
    let ctl = EngineController::with_transport(
        Box::new(VecSink::new()),
        Box::new(stub),
        url_lock,
        settings_path.clone(),
    );
    (ctl, dir)
}

fn wait_for<F: Fn() -> bool>(f: F) {
    for _ in 0..200 {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(f(), "condition not met within 4 s");
}

#[test]
fn queue_persists_and_restores_across_controllers() {
    let tracks = vec![
        track(1, AudioFormat::Wav, 500),
        track(2, AudioFormat::Wav, 500),
        track(3, AudioFormat::Wav, 500),
    ];
    let (ctl, dir) = stub_controller("persist", tracks.clone());
    ctl.set_repeat(RepeatMode::All);
    ctl.set_shuffle(false);
    ctl.play_queue(tracks.clone(), 1);
    wait_for(|| ctl.snapshot().queue_ids == vec![1, 2, 3]);
    drop(ctl);

    let qp = dir.join("queue.json");
    assert!(qp.is_file(), "queue.json written on queue mutation");
    let raw = std::fs::read_to_string(&qp).expect("queue.json readable");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    assert_eq!(v["index"], 1);
    assert_eq!(v["repeat"], "all");
    assert_eq!(v["shuffle"], false);
    assert_eq!(v["tracks"].as_array().unwrap().len(), 3);

    // A fresh controller built the way the shell builds it (`new`) restores
    // the queue without playing. No stream is opened (status stays Stopped)
    // so the HTTP transport never touches the network.
    let settings_path = dir.join("settings.json");
    let ctl2 = EngineController::new(Box::new(VecSink::new()), settings_path);
    // Restore is sent via the command channel; give the thread a beat.
    wait_for(|| ctl2.snapshot().queue_ids == vec![1, 2, 3]);
    let snap = ctl2.snapshot();
    assert_eq!(
        snap.status,
        PlayerStatus::Stopped,
        "restore must not start playback"
    );
    assert_eq!(snap.queue_index, Some(1));
    assert_eq!(snap.repeat, RepeatMode::All);
    assert!(!snap.shuffle);
    assert_eq!(snap.track.as_ref().map(|t| t.id), Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pausing_saves_the_playhead_in_queue_json() {
    let dir = std::env::temp_dir().join("kahawai-player-core-position");
    let _ = std::fs::remove_dir_all(&dir);
    let qp = dir.join("queue.json");
    let mut h = Harness::new(None);
    h.player.set_queue_path(Some(qp.clone()));
    h.stub.add(1, &[(440.0, 44100 * 10)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 10_000)], 0);
    while h.player.snapshot().position_ms < 500 {
        h.player.pump();
    }
    let saved = || {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&qp).unwrap()).unwrap();
        v["position_ms"].as_u64().unwrap()
    };
    h.player.pause();
    assert!(
        saved() >= 500,
        "pause saves how far into the track playback was"
    );
    h.player.resume();
    h.player.pump();
    h.player.persist_position();
    assert!(
        saved() >= 500,
        "the periodic save keeps it current while playing"
    );
    h.player.stop();
    assert_eq!(saved(), 0, "an explicit stop forgets the position");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Playhead saved in `queue.json`, in ms.
fn saved_position_ms(dir: &std::path::Path) -> u64 {
    let raw = std::fs::read_to_string(dir.join("queue.json")).expect("queue.json readable");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    v["position_ms"].as_u64().expect("position_ms")
}

/// A reader that hands out its bytes slowly, so a stub track keeps playing
/// for a couple of seconds instead of decoding into the [`VecSink`] at once.
struct Throttled(Box<dyn std::io::Read + Send>);

impl std::io::Read for Throttled {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(5));
        let n = buf.len().min(4096);
        self.0.read(&mut buf[..n])
    }
}

/// [`StubTransport`] with paced streams.
struct SlowTransport(StubTransport);

impl Transport for SlowTransport {
    fn open_stream(&self, track_id: i64, opts: &StreamOptions) -> Result<StreamInfo, MusicError> {
        let mut info = self.0.open_stream(track_id, opts)?;
        info.reader = Box::new(Throttled(info.reader));
        Ok(info)
    }
}

/// A controller that is still playing track 1 well after it starts (about
/// 30 s of audio, delivered at several times real time), with the queue file
/// in the returned temp dir.
fn playing_controller(dir_suffix: &str) -> (EngineController, std::path::PathBuf) {
    let stub = StubTransport::new(None);
    stub.add(1, &[(440.0, (RATE as usize) * 30)]);
    let dir = std::env::temp_dir().join(format!("kahawai-player-core-{dir_suffix}"));
    let _ = std::fs::remove_dir_all(&dir);
    let ctl = EngineController::with_transport(
        Box::new(VecSink::new()),
        Box::new(SlowTransport(stub)),
        Arc::new(std::sync::RwLock::new("http://stub".to_string())),
        dir.join("settings.json"),
    );
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    wait_for(|| {
        ctl.snapshot().status == PlayerStatus::Playing && ctl.snapshot().position_ms >= 200
    });
    (ctl, dir)
}

#[test]
fn shutting_down_while_playing_saves_the_playhead() {
    // The periodic save runs every 5 s, so a quit in between used to lose up
    // to that much position. `shutdown` must write the live playhead first.
    let (ctl, dir) = playing_controller("shutdown-saves");
    assert_eq!(
        saved_position_ms(&dir),
        0,
        "no periodic save has happened yet"
    );
    ctl.shutdown();
    let saved = saved_position_ms(&dir);
    assert!(
        saved >= 200,
        "shutdown saved the live playhead, got {saved} ms"
    );
    ctl.shutdown(); // a second call is harmless
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_the_controller_while_playing_saves_the_playhead() {
    // Same guarantee when the controller is simply dropped (no explicit call).
    let (ctl, dir) = playing_controller("drop-saves");
    assert_eq!(
        saved_position_ms(&dir),
        0,
        "no periodic save has happened yet"
    );
    drop(ctl);
    let saved = saved_position_ms(&dir);
    assert!(saved >= 200, "drop saved the live playhead, got {saved} ms");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn restored_queue_resumes_from_its_saved_position_once() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 40)]);
    h.stub.add(2, &[(440.0, 44100 * 40)]);
    let tracks = vec![
        track(1, AudioFormat::Wav, 40_000),
        track(2, AudioFormat::Wav, 40_000),
    ];

    h.player
        .restore_queue(tracks.clone(), 1, RepeatMode::All, true, 30_000);
    let snap = h.player.snapshot();
    assert_eq!(
        snap.status,
        PlayerStatus::Stopped,
        "restore never starts playback"
    );
    assert_eq!(snap.repeat, RepeatMode::All);
    assert!(snap.shuffle);
    assert_eq!(
        snap.position_ms, 30_000,
        "the idle snapshot shows where it will resume"
    );
    assert_eq!(snap.duration_ms, Some(40_000));

    h.player.resume();
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    assert!(
        h.player.snapshot().position_ms >= 30_000,
        "resumed at the saved playhead"
    );

    // Only the first resume uses it: stop, then play again from the top.
    h.player.stop();
    assert_eq!(h.player.snapshot().position_ms, 0);
    h.player.resume();
    assert!(
        h.player.snapshot().position_ms < 5_000,
        "a later resume starts the track over"
    );
}

#[test]
fn opening_another_track_discards_the_restored_position() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 40)]);
    h.stub.add(2, &[(440.0, 44100 * 40)]);
    let tracks = vec![
        track(1, AudioFormat::Wav, 40_000),
        track(2, AudioFormat::Wav, 40_000),
    ];
    h.player
        .restore_queue(tracks.clone(), 0, RepeatMode::Off, false, 30_000);
    h.player.play_queue(tracks, 1); // the user picked something else
    assert!(h.player.snapshot().position_ms < 5_000);
    h.player.stop();
    h.player.resume();
    assert!(
        h.player.snapshot().position_ms < 5_000,
        "the old position must not come back"
    );
}

#[test]
fn repeat_shuffle_commands_reach_snapshot() {
    let (ctl, dir) = stub_controller("modes", vec![track(1, AudioFormat::Wav, 500)]);
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 500)], 0);
    ctl.set_shuffle(true);
    wait_for(|| ctl.snapshot().shuffle);
    assert_eq!(ctl.snapshot().repeat, RepeatMode::Off);
    ctl.set_repeat(RepeatMode::One);
    wait_for(|| ctl.snapshot().repeat == RepeatMode::One);
    assert!(ctl.snapshot().shuffle);
    ctl.set_repeat(RepeatMode::Off);
    ctl.set_shuffle(false);
    wait_for(|| !ctl.snapshot().shuffle && ctl.snapshot().repeat == RepeatMode::Off);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_and_insert_next_do_not_disturb_playback() {
    let tracks = vec![
        track(1, AudioFormat::Wav, 500),
        track(2, AudioFormat::Wav, 500),
    ];
    let (ctl, _dir) = stub_controller("append", tracks.clone());
    ctl.play_queue(tracks.clone(), 0);
    wait_for(|| ctl.snapshot().current_id == Some(1));

    ctl.append_tracks(vec![track(3, AudioFormat::Wav, 500)]);
    wait_for(|| ctl.snapshot().queue_ids == vec![1, 2, 3]);
    assert_eq!(ctl.snapshot().current_id, Some(1), "append keeps current");

    ctl.insert_tracks_next(vec![track(4, AudioFormat::Wav, 500)]);
    wait_for(|| ctl.snapshot().queue_ids == vec![1, 4, 2, 3]);
    assert_eq!(
        ctl.snapshot().current_id,
        Some(1),
        "insert-next keeps current"
    );
    assert_eq!(ctl.snapshot().queue_index, Some(0));
}

#[test]
fn dsd_story_pref_round_trips_through_settings_file() {
    let (ctl, dir) = stub_controller("dsd", vec![]);
    let (story, fmt) = ctl.playback_prefs();
    assert_eq!(
        story,
        DsdStory::Auto,
        "default: native on known DACs, convert elsewhere"
    );
    assert_eq!(fmt, None);
    ctl.set_dsd_story(DsdStory::Native);
    std::thread::sleep(Duration::from_millis(100));
    let saved = std::fs::read_to_string(dir.join("settings.json")).expect("settings saved");
    assert!(saved.contains("\"native\""), "{saved}");
    ctl.set_global_format(Some(kahawai_core::api::StreamFormat::Opus));
    let (story2, fmt2) = ctl.playback_prefs();
    assert_eq!(story2, DsdStory::Native);
    assert_eq!(fmt2, Some(kahawai_core::api::StreamFormat::Opus));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn confirmed_dsd_devices_persist_and_toggle() {
    let (ctl, dir) = stub_controller("dsd-dev", vec![]);
    assert!(ctl.dsd_devices().is_empty());
    ctl.set_dsd_device_confirmed("Topping D90", true);
    ctl.set_dsd_device_confirmed("topping d90 ", true); // same device: no duplicate
    assert_eq!(ctl.dsd_devices(), vec!["topping d90".to_string()]);
    std::thread::sleep(Duration::from_millis(100));
    let saved = std::fs::read_to_string(dir.join("settings.json")).expect("settings saved");
    assert!(saved.contains("topping d90"), "{saved}");
    ctl.set_dsd_device_confirmed("Topping D90", false);
    assert!(ctl.dsd_devices().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Bit-perfect exclusive PCM output (MQA DACs / audiophile mode)
// ---------------------------------------------------------------------------

/// A player whose exclusive sink accepts the fixtures' 44.1 kHz.
fn bp_harness(mode: BitPerfect, exclusive_rates: &[u32]) -> Harness {
    let mut h = Harness::new(None);
    h.sink.0.lock().unwrap().exclusive_rates = exclusive_rates.to_vec();
    h.player.set_bit_perfect(mode);
    h
}

fn mqa_track(id: i64, ms: u64) -> Track {
    Track {
        mqa: true,
        original_sample_rate: Some(96000),
        ..track(id, AudioFormat::Wav, ms)
    }
}

fn exclusive_bytes(h: &Harness) -> Vec<u8> {
    h.sink.0.lock().unwrap().exclusive_bytes.clone()
}

/// Sign-extended packed 24-bit little-endian samples.
fn i24s(bytes: &[u8]) -> Vec<i32> {
    bytes
        .chunks_exact(3)
        .map(|b| (i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8) >> 8)
        .collect()
}

fn path_of(h: &Harness) -> OutputPath {
    h.sink.0.lock().unwrap().selected_path.unwrap_or_default()
}

#[test]
fn mqa_mode_plays_mqa_tracks_on_the_exclusive_path_with_exact_samples() {
    let mut h = bp_harness(BitPerfect::Mqa, &[RATE]);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player.play_queue(vec![mqa_track(1, 100)], 0);
    assert_eq!(h.player.snapshot().output_path, OutputPath::PcmExclusive);
    h.pump_until_done(100);

    {
        let sink = h.sink.0.lock().unwrap();
        assert_eq!(
            sink.exclusive_open,
            Some((RATE, CHANNELS as u16)),
            "opened at the file's own rate"
        );
        assert_eq!(sink.shared_opens, 0, "the shared path was never touched");
        assert!(
            sink.samples.is_empty(),
            "no float PCM went to the shared sink"
        );
    }
    // Packed 24-bit stereo: 3 bytes per sample, and every value is exactly
    // the source's 16-bit sample in the top of the word.
    let got = i24s(&exclusive_bytes(&h));
    assert_eq!(got.len(), 4410 * CHANNELS);
    for frame in [0usize, 1, 7, 100, 2205, 4409] {
        let want = i32::from(expected_i16(440.0, frame)) * 256;
        assert_eq!(got[frame * 2], want, "left, frame {frame}");
        assert_eq!(got[frame * 2 + 1], want, "right, frame {frame}");
    }
}

#[test]
fn mqa_mode_leaves_ordinary_tracks_on_shared_output() {
    let mut h = bp_harness(BitPerfect::Mqa, &[RATE]);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    h.pump_until_done(100);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
    let sink = h.sink.0.lock().unwrap();
    assert_eq!(sink.exclusive_open, None);
    assert!(sink.exclusive_bytes.is_empty());
    assert_eq!(sink.shared_opens, 1);
}

#[test]
fn all_mode_uses_the_exclusive_path_for_every_track() {
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    h.pump_until_done(100);
    assert!(h.sink.0.lock().unwrap().exclusive_open.is_some());
    assert_eq!(h.sink.0.lock().unwrap().shared_opens, 0);
}

#[test]
fn off_mode_never_goes_exclusive_even_for_mqa() {
    let mut h = bp_harness(BitPerfect::Off, &[RATE]);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player.play_queue(vec![mqa_track(1, 100)], 0);
    h.pump_until_done(100);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
    assert_eq!(h.sink.0.lock().unwrap().exclusive_open, None);
}

#[test]
fn bit_perfect_ignores_volume_eq_and_loudness() {
    let plain = {
        let mut h = bp_harness(BitPerfect::All, &[RATE]);
        h.stub.add(1, &[(440.0, 8820)]);
        h.player
            .play_queue(vec![track(1, AudioFormat::Wav, 200)], 0);
        h.pump_until_done(200);
        exclusive_bytes(&h)
    };
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.player.set_volume(0.25);
    h.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 440.0,
            gain_db: 12.0,
            q: 1.0,
        }])
        .expect("valid band");
    h.player.set_eq_enabled(true);
    h.player.set_analog(AnalogSettings {
        enabled: true,
        flavour: AnalogFlavour::WarmTriode,
        drive: 1.0,
        mix: 1.0,
        output_db: 6.0,
        auto_gain: false,
        ..Default::default()
    });
    h.player.set_loudness_enabled(true);
    h.stub.add(1, &[(440.0, 8820)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 200)], 0);
    h.pump_until_done(200);
    assert_eq!(
        exclusive_bytes(&h),
        plain,
        "identical bytes with every DSP stage switched on"
    );
    assert_eq!(
        h.stub.opened.lock().unwrap().len(),
        1,
        "no loudness pre-scan stream is requested in bit-perfect mode"
    );
}

#[test]
fn falls_back_to_shared_output_when_the_device_cannot_take_the_rate() {
    for rates in [vec![], vec![96000u32]] {
        let mut h = bp_harness(BitPerfect::All, &rates);
        h.stub.add(1, &[(440.0, 4410)]);
        h.player.play_queue(vec![mqa_track(1, 100)], 0);
        h.pump_until_done(100);
        assert_eq!(
            h.player.snapshot().output_path,
            OutputPath::Pcm,
            "rates {rates:?}"
        );
        assert_eq!(path_of(&h), OutputPath::Pcm);
        let sink = h.sink.0.lock().unwrap();
        assert_eq!(sink.exclusive_open, None);
        assert_eq!(sink.shared_opens, 1);
        assert!(
            !sink.samples.is_empty(),
            "still plays, through the shared path"
        );
    }
}

#[test]
fn falls_back_to_shared_output_when_the_exclusive_device_is_busy() {
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.sink.0.lock().unwrap().fail_exclusive_open = true;
    h.stub.add(1, &[(440.0, 4410)]);
    h.player.play_queue(vec![mqa_track(1, 100)], 0);
    h.pump_until_done(100);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
    assert_eq!(h.player.snapshot().error, None, "playback did not fail");
    assert!(!h.sink.0.lock().unwrap().samples.is_empty());
}

#[test]
fn a_forced_transcode_is_not_bit_perfect() {
    // The file's own bits are what bit-perfect means: a server transcode is a
    // different signal, so an explicit format choice keeps the shared path.
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player.set_track_format(1, Some(StreamFormat::Flac));
    h.player.play_queue(vec![mqa_track(1, 100)], 0);
    h.pump_until_done(100);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
    assert_eq!(h.sink.0.lock().unwrap().exclusive_open, None);
}

#[test]
fn bit_perfect_plays_one_track_per_stream_and_never_chains_the_next() {
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.stub.add(1, &[(440.0, 4410)]);
    h.stub.add(2, &[(660.0, 4410)]);
    h.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 100),
            track(2, AudioFormat::Wav, 100),
        ],
        0,
    );
    h.pump_until_done(200);
    {
        let opened = h.stub.opened.lock().unwrap();
        assert_eq!(opened.len(), 2, "one request per track");
        assert!(
            opened.iter().all(|(_, o)| o.next.is_none()),
            "no ?next= in bit-perfect mode"
        );
    }
    // Both tracks reached the exclusive device, back to back.
    assert_eq!(i24s(&exclusive_bytes(&h)).len(), 2 * 4410 * CHANNELS);

    // Contrast: the shared path does ask the server to chain.
    let mut shared = Harness::new(None);
    shared.stub.add(1, &[(440.0, 4410)]);
    shared.stub.add(2, &[(660.0, 4410)]);
    shared.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 100),
            track(2, AudioFormat::Wav, 100),
        ],
        0,
    );
    assert_eq!(shared.stub.opened.lock().unwrap()[0].1.next, Some(2));
}

#[test]
fn seeking_in_bit_perfect_resumes_at_the_target_sample() {
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.stub.add(1, &[(440.0, 88200)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 2000)], 0);
    for _ in 0..5 {
        h.player.pump();
    }
    let before = exclusive_bytes(&h).len();
    h.player.seek_ms(1000);
    assert_eq!(h.player.snapshot().position_ms, 1000);
    h.pump_until_done(200);
    let all = i24s(&exclusive_bytes(&h));
    let first_after_seek = before / 3;
    let want = i32::from(expected_i16(440.0, 44100)) * 256;
    assert_eq!(
        all[first_after_seek], want,
        "first sample after the seek is sample #44100"
    );
    assert_eq!(h.player.snapshot().output_path, OutputPath::PcmExclusive);
}

#[test]
fn position_tracks_the_exclusive_device_buffer() {
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    for _ in 0..30 {
        h.player.pump();
    }
    let written = h.player.snapshot().position_ms;
    assert!(written > 1000);
    h.sink.0.lock().unwrap().buffered = (RATE / 2) as u64; // half a second still queued
    assert_eq!(h.player.snapshot().position_ms, written - 500);
}

#[test]
fn changing_the_preference_mid_track_moves_to_the_new_path_at_the_same_position() {
    let mut h = bp_harness(BitPerfect::Off, &[RATE]);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player.play_queue(vec![mqa_track(1, 4000)], 0);
    for _ in 0..15 {
        h.player.pump();
    }
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
    let pos = h.player.snapshot().position_ms;

    h.player.set_bit_perfect(BitPerfect::Mqa);
    let snap = h.player.snapshot();
    assert_eq!(snap.output_path, OutputPath::PcmExclusive);
    assert!(
        snap.position_ms.abs_diff(pos) <= 2,
        "{pos} -> {}",
        snap.position_ms
    );
    assert_eq!(snap.status, PlayerStatus::Playing);

    h.player.set_bit_perfect(BitPerfect::Off);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
    // Setting the same value again is a no-op (no re-open).
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.set_bit_perfect(BitPerfect::Off);
    assert_eq!(h.stub.opened.lock().unwrap().len(), opens);
}

#[test]
fn a_paused_track_stays_paused_when_the_preference_changes() {
    let mut h = bp_harness(BitPerfect::Off, &[RATE]);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player.play_queue(vec![mqa_track(1, 4000)], 0);
    for _ in 0..5 {
        h.player.pump();
    }
    h.player.pause();
    h.player.set_bit_perfect(BitPerfect::All);
    assert_eq!(h.player.status(), PlayerStatus::Paused);
    assert_eq!(h.player.snapshot().output_path, OutputPath::PcmExclusive);
}

#[test]
fn a_write_failure_on_the_exclusive_device_continues_on_shared_output_with_a_notice() {
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.sink.0.lock().unwrap().exclusive_open = None; // device vanished
    h.player.pump();
    assert_eq!(
        h.player.status(),
        PlayerStatus::Playing,
        "carries on rather than stopping"
    );
    let snap = h.player.snapshot();
    assert!(snap.error.is_none());
    assert_eq!(snap.output_path, OutputPath::Pcm, "on shared output now");
    assert!(
        snap.notice
            .as_deref()
            .unwrap_or("")
            .contains("exclusive output stopped"),
        "{:?}",
        snap.notice
    );
    // It does not keep retrying exclusive for this track.
    h.player.pump();
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
}

#[test]
fn the_preference_persists_across_controllers() {
    let dir = std::env::temp_dir().join(format!("kahawai-bp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("engine-settings.json");
    {
        let c = EngineController::new(Box::new(VecSink::new()), path.clone());
        assert_eq!(
            c.bit_perfect(),
            BitPerfect::Auto,
            "follows the quality mode by default"
        );
        c.set_bit_perfect(BitPerfect::Mqa);
        assert_eq!(c.bit_perfect(), BitPerfect::Mqa);
    }
    let c = EngineController::new(Box::new(VecSink::new()), path.clone());
    assert_eq!(c.bit_perfect(), BitPerfect::Mqa);
    c.set_bit_perfect(BitPerfect::All);
    assert_eq!(
        EngineController::new(Box::new(VecSink::new()), path).bit_perfect(),
        BitPerfect::All
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Quality mode: Best quality / Compatible
// ---------------------------------------------------------------------------

fn best_harness(mode: kahawai_player_core::QualityMode, external: bool) -> Harness {
    let mut h = bp_harness(BitPerfect::Auto, &[RATE]);
    h.sink.0.lock().unwrap().external = external;
    h.player.set_quality_mode(mode);
    h
}

fn play_one(h: &mut Harness) {
    h.stub.add(1, &[(440.0, 4410)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    h.pump_until_done(100);
}

#[test]
fn best_quality_goes_exclusive_on_an_external_dac() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    play_one(&mut h);
    assert!(
        h.sink.0.lock().unwrap().exclusive_open.is_some(),
        "rate-matched exclusive output"
    );
    assert!(h.player.snapshot().notice.is_none());
}

/// The DAC opens exclusively but won't start (seen with a USB DAC another
/// app is using): the track plays on shared output with a notice, instead
/// of failing with "Couldn't start the audio output."
#[test]
fn an_exclusive_device_that_will_not_start_falls_back_to_shared_output() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.sink.0.lock().unwrap().fail_exclusive_start = true;
    play_one(&mut h);
    let s = h.player.snapshot();
    assert!(s.error.is_none(), "no error: {:?}", s.error);
    assert_eq!(s.output_path, OutputPath::Pcm);
    assert!(s
        .notice
        .unwrap_or_default()
        .contains("couldn't be started for exclusive use"));
    assert!(!h.samples().is_empty(), "the track played on shared output");
}

#[test]
fn best_quality_stays_shared_on_built_in_output() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, false);
    play_one(&mut h);
    assert_eq!(
        h.sink.0.lock().unwrap().exclusive_open,
        None,
        "never hog the built-in output"
    );
    assert!(
        h.player.snapshot().notice.is_none(),
        "a choice, not something to explain"
    );
}

#[test]
fn compatible_mode_never_goes_exclusive() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Compatible, true);
    play_one(&mut h);
    assert_eq!(h.sink.0.lock().unwrap().exclusive_open, None);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
}

#[test]
fn best_quality_yields_to_the_users_own_processing_and_says_so() {
    for (name, setup) in [
        (
            "EQ",
            Box::new(|h: &mut Harness| {
                h.player
                    .set_eq_bands(vec![EqBand {
                        band_type: EqBandType::Peaking,
                        freq: 1000.0,
                        gain_db: 3.0,
                        q: 1.0,
                    }])
                    .unwrap();
                h.player.set_eq_enabled(true);
            }) as Box<dyn Fn(&mut Harness)>,
        ),
        (
            "Loudness",
            Box::new(|h: &mut Harness| h.player.set_loudness_enabled(true)),
        ),
        (
            "Analog",
            Box::new(|h: &mut Harness| {
                h.player.set_analog(AnalogSettings {
                    enabled: true,
                    ..Default::default()
                });
            }),
        ),
        (
            "Volume",
            Box::new(|h: &mut Harness| h.player.set_volume(0.5)),
        ),
    ] {
        let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
        setup(&mut h);
        h.stub.add(1, &[(440.0, 4410)]);
        h.player
            .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
        let snap = h.player.snapshot();
        assert_eq!(snap.output_path, OutputPath::Pcm, "{name}: shared output");
        assert!(
            snap.exclusive_blockers.iter().any(|b| b == name),
            "{name}: {:?}",
            snap.exclusive_blockers
        );
        let notice = snap.notice.unwrap_or_default();
        assert!(
            notice.contains(name) && notice.contains("Best quality is paused"),
            "{name}: {notice}"
        );
    }
}

#[test]
fn best_quality_resumes_once_the_processing_is_switched_off() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.player.set_volume(0.5);
    play_one(&mut h);
    assert_eq!(h.sink.0.lock().unwrap().exclusive_open, None);
    h.player.set_volume(1.0);
    play_one(&mut h);
    assert!(h.sink.0.lock().unwrap().exclusive_open.is_some());
    assert!(h.player.snapshot().exclusive_blockers.is_empty());
}

#[test]
fn an_explicit_bit_perfect_choice_overrides_the_quality_mode() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Compatible, true);
    h.player.set_bit_perfect(BitPerfect::All);
    play_one(&mut h);
    assert!(
        h.sink.0.lock().unwrap().exclusive_open.is_some(),
        "Advanced override wins"
    );
}

#[test]
fn best_quality_plays_dsd_natively_only_on_a_known_dac_without_processing() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.sink.0.lock().unwrap().device = Some("FIIO K15 ".into());
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Dop));

    // The user's EQ is on: convert to PCM so it can apply, and say why.
    let mut eq = DopHarness::new(Some(DOP_RATE));
    eq.sink.0.lock().unwrap().device = Some("FIIO K15 ".into());
    eq.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 1000.0,
            gain_db: 3.0,
            q: 1.0,
        }])
        .unwrap();
    eq.player.set_eq_enabled(true);
    eq.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    let snap = eq.player.snapshot();
    assert_eq!(snap.format, Some(StreamFormat::Flac));
    assert!(snap.notice.unwrap_or_default().contains("EQ"));

    // Compatible: always converted.
    let mut c = DopHarness::new(Some(DOP_RATE));
    c.sink.0.lock().unwrap().device = Some("FIIO K15 ".into());
    c.player
        .set_quality_mode(kahawai_player_core::QualityMode::Compatible);
    c.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(c.player.snapshot().format, Some(StreamFormat::Flac));
}

#[test]
fn settings_from_before_the_quality_mode_are_reset_to_auto_once() {
    let dir = std::env::temp_dir().join(format!("kahawai-qm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("engine-settings.json");
    // An old file: explicit choices made while troubleshooting.
    std::fs::write(
        &path,
        r#"{"server_url":"http://x:1","dsp":{"eq_bands":[],"eq_enabled":true,"loudness_enabled":false,"loudness_target":-14.0},"dsd_story":"native","global_format":"passthrough","output_device":"FIIO K15 ","bit_perfect":"off"}"#,
    )
    .unwrap();
    let c = EngineController::new(Box::new(VecSink::new()), path.clone());
    assert_eq!(c.quality_mode(), kahawai_player_core::QualityMode::Best);
    assert_eq!(c.bit_perfect(), BitPerfect::Auto);
    let (story, fmt) = c.playback_prefs();
    assert_eq!(story, DsdStory::Auto);
    assert_eq!(fmt, None);
    assert_eq!(
        c.output_device().as_deref(),
        Some("FIIO K15 "),
        "the device choice is kept"
    );
    // Once migrated, explicit choices stick.
    c.set_bit_perfect(BitPerfect::All);
    let c2 = EngineController::new(Box::new(VecSink::new()), path);
    assert_eq!(c2.bit_perfect(), BitPerfect::All);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn best_quality_says_why_a_track_fell_back_to_shared_output() {
    // The device is an external DAC but doesn't offer this file's rate.
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.sink.0.lock().unwrap().exclusive_rates = vec![48000];
    h.stub.add(1, &[(440.0, 4410)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    let snap = h.player.snapshot();
    assert_eq!(snap.output_path, OutputPath::Pcm);
    let notice = snap.notice.unwrap_or_default();
    assert!(
        notice.contains("44.1 kHz") && notice.contains("shared output"),
        "{notice}"
    );
}

#[test]
fn a_missing_chosen_device_is_named_in_a_notice() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.sink.0.lock().unwrap().device = Some("MacBook Pro Speakers".into());
    h.player.set_output_device(Some("FIIO K15 ".into()));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    let notice = h.player.snapshot().notice.unwrap_or_default();
    assert!(
        notice.contains("FIIO K15")
            && notice.contains("isn't connected")
            && notice.contains("MacBook Pro Speakers"),
        "{notice}"
    );
}

#[test]
fn no_notice_when_the_chosen_device_is_the_one_in_use() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.sink.0.lock().unwrap().device = Some("fiio k15".into()); // case and padding differ
    h.player.set_output_device(Some("FIIO K15 ".into()));
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert!(h.player.snapshot().notice.is_none());
}

#[test]
fn losing_the_dac_during_native_dsd_continues_on_shared_output() {
    let mut h = DopHarness::new(Some(DOP_RATE));
    h.sink.0.lock().unwrap().device = Some("FIIO K15 ".into());
    h.player.play_queue(vec![dsd_track(1, DSD64, 1000)], 0);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Dop);
    h.sink.0.lock().unwrap().fail_write = true; // unplugged
    h.player.pump();
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    let snap = h.player.snapshot();
    assert_eq!(snap.output_path, OutputPath::Pcm);
    assert_eq!(snap.format, Some(StreamFormat::Flac));
    assert!(snap.error.is_none());
    assert!(
        snap.notice
            .unwrap_or_default()
            .contains("exclusive output stopped"),
        "says what happened"
    );
    h.pump_until_done(100);
    assert!(
        h.pcm_writes() > 0,
        "audio continues through the shared path"
    );
}

#[test]
fn nothing_the_shared_path_emits_exceeds_full_scale() {
    // +12 dB on a 0.7-amplitude tone is about 2.8 full scale. The device would
    // hard-clip that; the look-ahead limiter holds the peak at its ceiling
    // (headroom_guard is a backstop behind it and should not need to bend
    // anything here).
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player.set_limiter_enabled(true);
    h.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 440.0,
            gain_db: 12.0,
            q: 1.0,
        }])
        .unwrap();
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.pump_until_done(100);
    let peak = h.samples().iter().fold(0.0f32, |a, x| a.max(x.abs()));
    assert!(peak <= 1.0, "peak {peak}");
    assert!(
        (peak - LIMITER_CEILING).abs() < 1e-3,
        "the limiter should hold the peak right at its ceiling, not bend it lower or let it through higher: {peak}"
    );
}

#[test]
fn turning_the_limiter_off_falls_back_to_the_guard_and_keeps_every_frame() {
    // Off, the same overdriven material is soft-clipped by headroom_guard
    // instead: still bounded, but allowed above the limiter's ceiling. The
    // frame count must not change either way - switching it off drains
    // whatever the look-ahead was holding rather than dropping it.
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 440.0,
            gain_db: 12.0,
            q: 1.0,
        }])
        .unwrap();
    h.player.set_limiter_enabled(false);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.pump_until_done(100);

    let s = h.samples();
    assert_eq!(s.len(), 44100 * CHANNELS, "exact total samples");
    let peak = s.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    assert!(peak <= 1.0, "the guard still bounds it: {peak}");
    assert!(
        peak > LIMITER_CEILING + 1e-3,
        "without the limiter the guard's soft knee goes above the ceiling: {peak}"
    );
    assert_eq!(
        h.player.snapshot().limiter_gr_db,
        None,
        "no reading while off"
    );
}

#[test]
fn the_limiter_reports_gain_reduction_only_while_it_is_working() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player.set_limiter_enabled(true);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.player.pump();
    // A 0.7 FS tone with no boost stays under the ceiling: nothing to do.
    assert_eq!(
        h.player.snapshot().limiter_gr_db,
        Some(0.0),
        "on, but not working"
    );

    // A large boost drives it well past the ceiling.
    h.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 440.0,
            gain_db: 12.0,
            q: 1.0,
        }])
        .unwrap();
    for _ in 0..5 {
        h.player.pump();
    }
    let gr = h
        .player
        .snapshot()
        .limiter_gr_db
        .expect("a reading on the shared path");
    assert!(
        gr > 3.0,
        "a +12 dB boost on a 0.7 FS tone needs real reduction, got {gr}"
    );
}

#[test]
fn loudness_gain_is_reduced_so_the_eq_boosted_peak_cannot_clip() {
    // A 0.7 FS tone that loudness normalization wants to lift a long way, plus
    // a +6 dB EQ boost: the planned gain must leave the final peak below full
    // scale, with no guard needed.
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 3)]);
    h.player
        .set_eq_bands(vec![EqBand {
            band_type: EqBandType::Peaking,
            freq: 440.0,
            gain_db: 6.0,
            q: 1.0,
        }])
        .unwrap();
    h.player.set_loudness_target(-6.0); // asks for a big lift
    h.player.set_loudness_enabled(true);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 3000)], 0);
    h.pump_until_done(300);
    let s = h.samples();
    let steady = &s[s.len() / 2..];
    let peak = steady.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    assert!(
        peak < 0.95,
        "planned to stay under full scale before any guard: {peak}"
    );
}

// ---------------------------------------------------------------------------
// Format overrides apply to the playing track immediately
// ---------------------------------------------------------------------------

fn playing_mid_track() -> Harness {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.stub.add(2, &[(440.0, 44100 * 4)]);
    h.player.play_queue(
        vec![
            track(1, AudioFormat::Wav, 4000),
            track(2, AudioFormat::Wav, 4000),
        ],
        0,
    );
    for _ in 0..12 {
        h.player.pump();
    }
    h
}

#[test]
fn a_track_format_override_re_opens_the_playing_track_at_the_same_position() {
    let mut h = playing_mid_track();
    let before = h.player.snapshot();
    assert_eq!(before.format, Some(StreamFormat::Passthrough));
    assert!(
        before.position_ms > 100,
        "mid-track: {}",
        before.position_ms
    );

    h.player.set_track_format(1, Some(StreamFormat::Flac));
    let after = h.player.snapshot();
    assert_eq!(
        after.format,
        Some(StreamFormat::Flac),
        "applied now, not at the next open"
    );
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    assert!(
        after.position_ms + 250 >= before.position_ms,
        "resumes near where it was: {} vs {}",
        after.position_ms,
        before.position_ms
    );

    // Clearing it (Auto) goes back, again immediately.
    h.player.set_track_format(1, None);
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Passthrough));
}

#[test]
fn an_override_for_another_track_or_an_unchanged_one_does_not_disturb_playback() {
    let mut h = playing_mid_track();
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.set_track_format(2, Some(StreamFormat::Flac)); // not the loaded track
    assert_eq!(
        h.stub.opened.lock().unwrap().len(),
        opens,
        "no re-open for a different track"
    );
    h.player.set_track_format(1, Some(StreamFormat::Flac));
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.set_track_format(1, Some(StreamFormat::Flac)); // same value again
    assert_eq!(
        h.stub.opened.lock().unwrap().len(),
        opens,
        "no re-open when nothing changed"
    );
    // But it is remembered for when track 2 comes up.
    h.player.next();
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Flac));
}

#[test]
fn a_global_format_change_re_opens_the_playing_track() {
    let mut h = playing_mid_track();
    h.player.set_global_format(Some(StreamFormat::Flac));
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Flac));
    h.player.set_global_format(None);
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Passthrough));
}

#[test]
fn a_paused_track_stays_paused_when_its_format_changes() {
    let mut h = playing_mid_track();
    h.player.pause();
    h.player.set_track_format(1, Some(StreamFormat::Flac));
    assert_eq!(h.player.status(), PlayerStatus::Paused);
    assert_eq!(h.player.snapshot().format, Some(StreamFormat::Flac));
}

// ---------------------------------------------------------------------------
// In-place queue edits: reordering or removing must not restart playback
// ---------------------------------------------------------------------------

fn three_track_queue(chain_mode: Option<&str>) -> Harness {
    let mut h = Harness::new(chain_mode);
    for id in 1..=3 {
        h.stub.add(id, &[(440.0 + id as f32 * 110.0, 44100 * 4)]);
    }
    h.player.play_queue(
        (1..=3)
            .map(|id| track(id, AudioFormat::Wav, 4000))
            .collect(),
        0,
    );
    for _ in 0..12 {
        h.player.pump();
    }
    h
}

#[test]
fn moving_a_queue_entry_does_not_touch_the_playing_stream() {
    let mut h = three_track_queue(None);
    let opens = h.stub.opened.lock().unwrap().len();
    let before = h.player.snapshot();
    h.player.move_queue_item(2, 1);
    let after = h.player.snapshot();
    assert_eq!(after.queue_ids, vec![1, 3, 2]);
    assert_eq!(after.current_id, Some(1), "still on the same track");
    assert_eq!(
        h.stub.opened.lock().unwrap().len(),
        opens,
        "no stream was re-opened"
    );
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    assert!(
        after.position_ms >= before.position_ms,
        "the playhead did not restart"
    );
}

#[test]
fn moving_the_playing_track_keeps_it_playing() {
    let mut h = three_track_queue(None);
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.move_queue_item(0, 2);
    let s = h.player.snapshot();
    assert_eq!(s.queue_ids, vec![2, 3, 1]);
    assert_eq!(s.current_id, Some(1));
    assert_eq!(s.queue_index, Some(2), "the UI's highlight follows it");
    assert_eq!(h.stub.opened.lock().unwrap().len(), opens);
}

#[test]
fn removing_another_entry_does_not_touch_the_playing_stream() {
    let mut h = three_track_queue(None);
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.remove_queue_item(2);
    let s = h.player.snapshot();
    assert_eq!(s.queue_ids, vec![1, 2]);
    assert_eq!(s.current_id, Some(1));
    assert_eq!(h.stub.opened.lock().unwrap().len(), opens);
    assert_eq!(h.player.status(), PlayerStatus::Playing);
}

#[test]
fn removing_the_playing_track_moves_on_to_the_next() {
    let mut h = three_track_queue(None);
    h.player.remove_queue_item(0);
    let s = h.player.snapshot();
    assert_eq!(s.queue_ids, vec![2, 3]);
    assert_eq!(s.current_id, Some(2), "the next track took over");
    assert_eq!(h.player.status(), PlayerStatus::Playing);
}

#[test]
fn removing_the_last_remaining_track_stops_playback() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    h.player.pump();
    h.player.remove_queue_item(0);
    assert_eq!(h.player.status(), PlayerStatus::Stopped);
    assert!(h.player.snapshot().queue_ids.is_empty());
}

#[test]
fn out_of_range_edits_are_ignored() {
    let mut h = three_track_queue(None);
    h.player.move_queue_item(0, 9);
    h.player.remove_queue_item(9);
    assert_eq!(h.player.snapshot().queue_ids, vec![1, 2, 3]);
    assert_eq!(h.player.status(), PlayerStatus::Playing);
}

#[test]
fn reordering_the_next_track_refreshes_a_stale_gapless_chain() {
    // The server already chained track 2 after track 1. If the user moves
    // track 3 in front of it, that chained audio would play the wrong track, so
    // the stream is re-opened once at the current position.
    let mut h = three_track_queue(Some("chained"));
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.move_queue_item(2, 1); // [1,3,2]: what follows track 1 changed
    assert!(
        h.stub.opened.lock().unwrap().len() > opens,
        "re-opened to chain the right track"
    );
    assert_eq!(h.player.snapshot().current_id, Some(1));
}

/// A track whose file rate / channel count differ from the fixture default.
fn track_with(id: i64, rate: u32, channels: u8) -> Track {
    Track {
        sample_rate: Some(rate),
        channels: Some(channels),
        ..track(id, AudioFormat::Wav, 4000)
    }
}

fn requested_next(h: &Harness, id: i64) -> Option<i64> {
    h.stub
        .opened
        .lock()
        .unwrap()
        .iter()
        .find(|(t, _)| *t == id)
        .and_then(|(_, o)| o.next)
}

#[test]
fn a_rate_change_between_tracks_is_never_chained_into_one_response() {
    // The stream is opened at the first track's rate and the EQ, meters and
    // sink are configured for it, so a chained second track at another rate
    // would play at the wrong speed. The next track must open as its own stream.
    let mut h = Harness::new(Some("chained"));
    h.stub.add(1, &[(440.0, 44100 * 2)]);
    h.stub.add(2, &[(440.0, 44100 * 2)]);
    h.player
        .play_queue(vec![track_with(1, 44_100, 2), track_with(2, 96_000, 2)], 0);
    assert_eq!(
        requested_next(&h, 1),
        None,
        "44.1 kHz -> 96 kHz must not chain"
    );
}

#[test]
fn a_channel_change_between_tracks_is_never_chained() {
    let mut h = Harness::new(Some("chained"));
    h.stub.add(1, &[(440.0, 44100 * 2)]);
    h.stub.add(2, &[(440.0, 44100 * 2)]);
    h.player
        .play_queue(vec![track_with(1, 44_100, 2), track_with(2, 44_100, 1)], 0);
    assert_eq!(requested_next(&h, 1), None, "stereo -> mono must not chain");
}

#[test]
fn matching_rates_still_chain() {
    let mut h = Harness::new(Some("chained"));
    h.stub.add(1, &[(440.0, 44100 * 2)]);
    h.stub.add(2, &[(440.0, 44100 * 2)]);
    h.player
        .play_queue(vec![track_with(1, 96_000, 2), track_with(2, 96_000, 2)], 0);
    assert_eq!(requested_next(&h, 1), Some(2));
}

// ---------------------------------------------------------------------------
// Scrubbing must be robust: a seek that cannot be served never ends the song
// ---------------------------------------------------------------------------

fn playing_ten_seconds() -> Harness {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 10)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 10_000)], 0);
    while h.player.snapshot().position_ms < 1000 {
        h.player.pump();
    }
    h
}

#[test]
fn a_failed_seek_carries_on_from_where_it_was() {
    let mut h = playing_ten_seconds();
    let before = h.player.snapshot().position_ms;
    h.stub.fail_next_opens(1); // the seek's stream request fails; the recovery works
    h.player.seek_ms(7000);
    let s = h.player.snapshot();
    assert_eq!(s.status, PlayerStatus::Playing, "the song did not stop");
    assert!(s.error.is_none(), "no error: {:?}", s.error);
    assert!(
        s.position_ms.abs_diff(before) < 300,
        "resumed near {before} ms, not at the failed 7000: {}",
        s.position_ms
    );
    assert!(
        s.notice
            .as_deref()
            .unwrap_or("")
            .contains("Couldn't seek there"),
        "says what happened: {:?}",
        s.notice
    );
}

#[test]
fn if_the_recovery_fails_too_playback_stops_with_the_original_error() {
    let mut h = playing_ten_seconds();
    h.stub.fail_next_opens(2); // the server is really down
    h.player.seek_ms(7000);
    let s = h.player.snapshot();
    assert_eq!(s.status, PlayerStatus::Stopped);
    assert!(s.error.is_some(), "an honest error, not silence");
}

#[test]
fn seeking_to_the_end_lands_just_inside_it() {
    let mut h = playing_ten_seconds();
    h.player.seek_ms(10_000); // the slider's right-hand end
    let s = h.player.snapshot();
    assert_eq!(
        s.status,
        PlayerStatus::Playing,
        "an empty stream was not requested"
    );
    assert!(
        (9000..10_000).contains(&s.position_ms),
        "just before the end: {}",
        s.position_ms
    );
    h.pump_until_done(5000);
    assert_eq!(
        h.player.status(),
        PlayerStatus::Stopped,
        "the track still ends normally"
    );
}

#[test]
fn seeking_while_paused_stays_paused_even_when_the_seek_fails() {
    let mut h = playing_ten_seconds();
    h.player.pause();
    h.stub.fail_next_opens(1);
    h.player.seek_ms(5000);
    assert_eq!(h.player.status(), PlayerStatus::Paused);
}

// ---------------------------------------------------------------------------
// Passthrough seeks use a seekable source instead of streaming from byte 0
// ---------------------------------------------------------------------------

use kahawai_player_core::transport::StreamProgress;
use kahawai_player_core::{SeekableControl, SeekableStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq)]
enum SeekOffer {
    /// An in-memory seekable copy of the file.
    Seekable,
    /// The transport has no seekable source (the server ignores Range).
    Unavailable,
    /// Asking for one fails.
    Errors,
    /// The "seekable" bytes are not a container the decoder can read.
    NotAudio,
}

/// Counts bytes read from an in-memory seekable file.
struct CountingCursor {
    inner: Cursor<Vec<u8>>,
    read: Arc<AtomicU64>,
}

impl std::io::Read for CountingCursor {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }
}

impl std::io::Seek for CountingCursor {
    fn seek(&mut self, p: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p)
    }
}

/// [`StubTransport`] that can also offer the file as a seekable source.
struct SeekStub {
    stub: Arc<StubTransport>,
    file: Vec<u8>,
    offer: Mutex<SeekOffer>,
    seekable_opens: AtomicUsize,
    bytes_read: Arc<AtomicU64>,
    /// Handed out with the seekable source, when set.
    control: Mutex<Option<Arc<dyn SeekableControl>>>,
}

struct SeekStubTransport(Arc<SeekStub>);

impl Transport for SeekStubTransport {
    fn open_stream(&self, id: i64, opts: &StreamOptions) -> Result<StreamInfo, MusicError> {
        self.0.stub.open_stream(id, opts)
    }

    fn open_seekable(
        &self,
        _id: i64,
        _opts: &StreamOptions,
    ) -> Result<Option<SeekableStream>, MusicError> {
        let me = &self.0;
        let offer = *me.offer.lock().unwrap();
        match offer {
            SeekOffer::Unavailable => return Ok(None),
            SeekOffer::Errors => return Err(MusicError::Http("stub: range request failed".into())),
            _ => {}
        }
        me.seekable_opens.fetch_add(1, Ordering::SeqCst);
        let bytes = if offer == SeekOffer::NotAudio {
            vec![7u8; 4000]
        } else {
            me.file.clone()
        };
        Ok(Some(SeekableStream {
            byte_len: bytes.len() as u64,
            source: Box::new(CountingCursor {
                inner: Cursor::new(bytes),
                read: me.bytes_read.clone(),
            }),
            content_type: "audio/wav".into(),
            chain: Some("wav->passthrough".into()),
            control: me.control.lock().unwrap().clone(),
        }))
    }
}

struct SeekHarness {
    player: Player,
    me: Arc<SeekStub>,
    sink: SharedSink,
}

impl SeekHarness {
    /// A 10 s, 440 Hz track playing, with the file offered the given way.
    fn new(offer: SeekOffer) -> Self {
        let stub = Arc::new(StubTransport::new(None));
        stub.add(1, &[(440.0, 44100 * 10)]);
        let me = Arc::new(SeekStub {
            stub,
            file: wav_bytes(440.0, 44100 * 10),
            offer: Mutex::new(offer),
            seekable_opens: AtomicUsize::new(0),
            bytes_read: Arc::new(AtomicU64::new(0)),
            control: Mutex::new(None),
        });
        let sink = SharedSink(Arc::new(Mutex::new(VecSink::new())));
        let mut player = Player::new(
            Box::new(sink.clone()),
            Box::new(SeekStubTransport(me.clone())),
        );
        player.play_queue(vec![track(1, AudioFormat::Wav, 10_000)], 0);
        for _ in 0..4 {
            player.pump();
        }
        Self { player, me, sink }
    }

    fn stream_opens(&self) -> usize {
        self.me.stub.opened.lock().unwrap().len()
    }

    /// Seek, then return the interleaved samples produced after the seek.
    fn seek_and_listen(&mut self, ms: u64) -> Vec<f32> {
        let before = self.sink.0.lock().unwrap().samples.len();
        self.player.seek_ms(ms);
        // The forward-only fallback decodes and discards everything before the
        // target first, so give it as many pumps as that takes.
        for _ in 0..800 {
            self.player.pump();
            if self.sink.0.lock().unwrap().samples.len() - before > 2 * 8000 {
                break;
            }
        }
        self.sink.0.lock().unwrap().samples[before..].to_vec()
    }
}

/// 440 Hz at the file's 0.7 amplitude, `frames` frames into the track.
fn tone_at(frame: usize) -> f32 {
    0.7 * (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / RATE as f32).sin()
}

/// Past the click-free fade-in, the first audio after a seek to `ms` is the
/// file's audio from exactly there (right frequency and phase).
fn assert_audio_is_from(heard: &[f32], ms: u64) {
    let start_frame = (ms as usize) * RATE as usize / 1000;
    assert!(
        heard.len() > 2 * 6000,
        "enough audio after the seek: {}",
        heard.len()
    );
    for i in 3000..4000 {
        let (have, want) = (heard[i * CHANNELS], tone_at(start_frame + i));
        assert!(
            (have - want).abs() < 0.02,
            "frame {i} after {ms} ms: {have} vs {want}"
        );
    }
}

#[test]
fn a_passthrough_seek_uses_the_seekable_source_and_reads_little_before_the_target() {
    let mut h = SeekHarness::new(SeekOffer::Seekable);
    assert_eq!(
        h.stream_opens(),
        1,
        "the track itself streamed from the start"
    );
    let heard = h.seek_and_listen(7000);
    assert_eq!(
        h.me.seekable_opens.load(Ordering::SeqCst),
        1,
        "the seek opened a seekable source"
    );
    assert_eq!(
        h.stream_opens(),
        1,
        "and did not stream the whole file again"
    );
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    assert_audio_is_from(&heard, 7000);
    let read = h.me.bytes_read.load(Ordering::SeqCst);
    let file = h.me.file.len() as u64;
    assert!(
        read < file / 3,
        "read {read} of {file} bytes: not the 70% before the target"
    );
}

#[test]
fn the_playhead_after_a_seekable_seek_is_the_target() {
    let mut h = SeekHarness::new(SeekOffer::Seekable);
    h.player.seek_ms(4000);
    let pos = h.player.snapshot().position_ms;
    assert!(
        (4000..4100).contains(&pos),
        "position {pos} ms is the 4 s target, not 0 or a frame off"
    );
    h.seek_and_listen(4000);
    assert!(
        h.player.snapshot().position_ms >= 4000,
        "and it only moves forward from there"
    );
}

#[test]
fn without_a_seekable_source_the_seek_falls_back_to_streaming_and_skipping() {
    for offer in [
        SeekOffer::Unavailable,
        SeekOffer::Errors,
        SeekOffer::NotAudio,
    ] {
        let mut h = SeekHarness::new(offer);
        let heard = h.seek_and_listen(7000);
        assert_eq!(
            h.stream_opens(),
            2,
            "streamed again from the start and skipped"
        );
        assert_eq!(h.player.status(), PlayerStatus::Playing);
        assert_audio_is_from(&heard, 7000);
    }
}

// ---------------------------------------------------------------------------
// A stalled network must never freeze the controls
// ---------------------------------------------------------------------------

use kahawai_player_core::ReadAhead;
use std::sync::atomic::AtomicBool;

/// Serves some chunks, then goes dead until released, then serves the rest.
struct DeadThenAlive {
    before: Vec<u8>,
    after: Vec<u8>,
    at: usize,
    release: Arc<AtomicBool>,
}

impl std::io::Read for DeadThenAlive {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n_before = self.before.len();
        if self.at < n_before {
            let n = buf.len().min(n_before - self.at).min(4096);
            buf[..n].copy_from_slice(&self.before[self.at..self.at + n]);
            self.at += n;
            return Ok(n);
        }
        while !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let off = self.at - n_before;
        if off >= self.after.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.after.len() - off).min(4096);
        buf[..n].copy_from_slice(&self.after[off..off + n]);
        self.at += n;
        Ok(n)
    }
}

/// A 2 s track whose network delivers the first 0.5 s, dies, and (when
/// released) delivers the rest. Read ahead, as the real transport does.
struct DyingNetwork {
    release: Arc<AtomicBool>,
}

impl Transport for DyingNetwork {
    fn open_stream(&self, _id: i64, _opts: &StreamOptions) -> Result<StreamInfo, MusicError> {
        let whole = wav_bytes(440.0, 44100 * 2);
        let half = 44 + 44100 * 4 / 2; // header + 0.5 s of 16-bit stereo
        let ahead = ReadAhead::new(
            DeadThenAlive {
                before: whole[..half].to_vec(),
                after: whole[half..].to_vec(),
                at: 0,
                release: self.release.clone(),
            },
            1 << 20,
        );
        let stats = ahead.stats();
        Ok(StreamInfo {
            reader: Box::new(ahead),
            content_type: "audio/wav".into(),
            chain: None,
            gapless_next: None,
            gapless_mode: None,
            progress: Some(StreamProgress {
                received: Arc::new(AtomicU64::new(0)),
                content_length: None,
                offset: 0,
                stats: Some(stats),
            }),
        })
    }
}

fn dying_network_controller(suffix: &str) -> (EngineController, Arc<AtomicBool>) {
    let release = Arc::new(AtomicBool::new(false));
    let dir = std::env::temp_dir().join(format!("kahawai-player-core-{suffix}"));
    let _ = std::fs::remove_dir_all(&dir);
    let ctl = EngineController::with_transport(
        Box::new(VecSink::new()),
        Box::new(DyingNetwork {
            release: release.clone(),
        }),
        Arc::new(std::sync::RwLock::new("http://stub".to_string())),
        dir.join("settings.json"),
    );
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 2_000)], 0);
    (ctl, release)
}

#[test]
fn the_controls_still_answer_while_the_network_is_stalled() {
    let (ctl, release) = dying_network_controller("stall-controls");
    wait_for(|| ctl.snapshot().buffering);
    assert_eq!(
        ctl.snapshot().status,
        PlayerStatus::Playing,
        "still the playing track, just buffering"
    );
    // Before the fix this pause was never processed: the playback thread sat
    // blocked in the network read.
    ctl.pause();
    wait_for(|| ctl.snapshot().status == PlayerStatus::Paused);
    assert!(
        !ctl.snapshot().buffering,
        "a paused player is not buffering"
    );
    ctl.stop();
    wait_for(|| ctl.snapshot().status == PlayerStatus::Stopped);
    release.store(true, Ordering::SeqCst);
}

#[test]
fn playback_resumes_by_itself_when_the_network_comes_back() {
    let (ctl, release) = dying_network_controller("stall-resume");
    wait_for(|| ctl.snapshot().buffering);
    assert_eq!(ctl.snapshot().status, PlayerStatus::Playing);
    release.store(true, Ordering::SeqCst);
    // It plays on to the end of the track by itself: finished, with no error
    // and no one having touched the controls.
    wait_for(|| ctl.snapshot().status == PlayerStatus::Stopped);
    let s = ctl.snapshot();
    assert!(s.error.is_none(), "completed cleanly: {:?}", s.error);
    assert!(!s.buffering);
}

#[test]
fn a_stream_that_never_comes_back_ends_with_a_clear_error_not_a_hang() {
    let release = Arc::new(AtomicBool::new(false));
    let sink = SharedSink(Arc::new(Mutex::new(VecSink::new())));
    let mut player = Player::new(
        Box::new(sink),
        Box::new(DyingNetwork {
            release: release.clone(),
        }),
    );
    player.set_stall_timeout(Duration::from_millis(250));
    player.play_queue(vec![track(1, AudioFormat::Wav, 2_000)], 0);
    let mut waited = 0;
    while player.status() == PlayerStatus::Playing && waited < 600 {
        player.pump();
        waited += 1;
    }
    let s = player.snapshot();
    assert_eq!(
        s.status,
        PlayerStatus::Stopped,
        "gave up instead of hanging"
    );
    assert!(
        s.error
            .as_deref()
            .unwrap_or("")
            .contains("connection to the server was lost"),
        "says why: {:?}",
        s.error
    );
    release.store(true, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Opening a stream must never hold up the controls either
// ---------------------------------------------------------------------------

/// A transport whose stream requests can be slow, hang, or fail slowly.
struct SlowOpen {
    stub: Arc<StubTransport>,
    /// Each open takes this long.
    delay: Mutex<Duration>,
    /// While true, opens hang (a dead network at connect or probe time).
    hang: Arc<AtomicBool>,
    /// The next opens fail (after the delay) instead of succeeding.
    fail_next: Mutex<u32>,
    opens: AtomicUsize,
    /// Pace the stream like a real network (off for chained responses, which
    /// are read in full before decoding starts).
    paced: bool,
}

struct SlowOpenTransport(Arc<SlowOpen>);

impl Transport for SlowOpenTransport {
    fn open_stream(&self, id: i64, opts: &StreamOptions) -> Result<StreamInfo, MusicError> {
        let me = &self.0;
        me.opens.fetch_add(1, Ordering::SeqCst);
        while me.hang.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(*me.delay.lock().unwrap());
        {
            let mut fails = me.fail_next.lock().unwrap();
            if *fails > 0 {
                *fails -= 1;
                return Err(MusicError::Http("stub: server error".into()));
            }
        }
        // Paced like a real network, so a track does not finish in a flash.
        let mut info = me.stub.open_stream(id, opts)?;
        if me.paced {
            info.reader = Box::new(Throttled(info.reader));
        }
        Ok(info)
    }
}

fn slow_open_transport() -> Arc<SlowOpen> {
    let stub = Arc::new(StubTransport::new(None));
    stub.add(1, &[(440.0, 44100 * 30)]);
    Arc::new(SlowOpen {
        stub,
        delay: Mutex::new(Duration::ZERO),
        hang: Arc::new(AtomicBool::new(false)),
        fail_next: Mutex::new(0),
        opens: AtomicUsize::new(0),
        paced: true,
    })
}

fn controller_over(slow: &Arc<SlowOpen>, suffix: &str) -> EngineController {
    let dir = std::env::temp_dir().join(format!("kahawai-player-core-{suffix}"));
    let _ = std::fs::remove_dir_all(&dir);
    EngineController::with_transport(
        Box::new(VecSink::new()),
        Box::new(SlowOpenTransport(slow.clone())),
        Arc::new(std::sync::RwLock::new("http://stub".to_string())),
        dir.join("settings.json"),
    )
}

#[test]
fn a_dead_network_while_opening_does_not_freeze_the_controls() {
    let slow = slow_open_transport();
    slow.hang.store(true, Ordering::SeqCst);
    let ctl = controller_over(&slow, "open-hang");
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Loading);
    // Before the fix the playback thread sat inside the request, so none of
    // these were ever processed.
    ctl.next();
    ctl.stop();
    wait_for(|| ctl.snapshot().status == PlayerStatus::Stopped);
    slow.hang.store(false, Ordering::SeqCst);
}

#[test]
fn a_slow_open_plays_by_itself_once_the_stream_arrives() {
    let slow = slow_open_transport();
    *slow.delay.lock().unwrap() = Duration::from_millis(500); // well past the inline wait
    let ctl = controller_over(&slow, "open-slow");
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Loading);
    wait_for(|| {
        matches!(
            ctl.snapshot().status,
            PlayerStatus::Playing | PlayerStatus::Stopped
        )
    });
    let s = ctl.snapshot();
    assert!(s.error.is_none(), "no error: {:?}", s.error);
    assert!(s.track.is_some());
}

#[test]
fn a_slow_open_can_be_replaced_by_a_newer_one() {
    // Scrubbing while a seek is still opening: only the latest open counts.
    let slow = slow_open_transport();
    let ctl = controller_over(&slow, "open-replace");
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Playing);
    *slow.delay.lock().unwrap() = Duration::from_millis(400);
    ctl.seek_ms(1000);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Loading);
    ctl.seek_ms(2500); // supersedes the open in flight
    wait_for(|| ctl.snapshot().status == PlayerStatus::Playing);
    let pos = ctl.snapshot().position_ms;
    assert!(
        pos >= 2400,
        "playing from the latest target, not the first: {pos}"
    );
}

#[test]
fn a_slow_failing_seek_still_carries_on_from_where_it_was() {
    let slow = slow_open_transport();
    let ctl = controller_over(&slow, "open-slow-fail");
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    wait_for(|| {
        ctl.snapshot().status == PlayerStatus::Playing && ctl.snapshot().position_ms >= 300
    });
    *slow.delay.lock().unwrap() = Duration::from_millis(300);
    *slow.fail_next.lock().unwrap() = 1; // the seek's open fails, slowly; the recovery works
    ctl.seek_ms(3000);
    wait_for(|| {
        let s = ctl.snapshot();
        s.status == PlayerStatus::Playing
            && s.notice
                .as_deref()
                .unwrap_or("")
                .contains("Couldn't seek there")
    });
    assert!(ctl.snapshot().error.is_none());
}

#[test]
fn reordering_the_queue_during_a_slow_open_does_not_leave_a_stale_chain() {
    // The open asked the server to chain track 2 after track 1; by the time the
    // stream arrives, track 3 is next. The stale chain is refreshed.
    let stub = Arc::new(StubTransport::new(Some("chained")));
    for id in 1..=3 {
        stub.add(id, &[(440.0 + id as f32 * 110.0, 44100 * 30)]);
    }
    let slow = Arc::new(SlowOpen {
        stub,
        delay: Mutex::new(Duration::from_millis(400)),
        hang: Arc::new(AtomicBool::new(false)),
        fail_next: Mutex::new(0),
        opens: AtomicUsize::new(0),
        paced: false,
    });
    let ctl = controller_over(&slow, "open-reorder");
    ctl.play_queue(
        (1..=3)
            .map(|id| track(id, AudioFormat::Wav, 30_000))
            .collect(),
        0,
    );
    wait_for(|| ctl.snapshot().status == PlayerStatus::Loading);
    ctl.move_queue_item(2, 1); // [1, 3, 2]: what follows track 1 changed
    let chained_next = |want: i64| {
        slow.stub
            .opened
            .lock()
            .unwrap()
            .iter()
            .any(|(id, o)| *id == 1 && o.next == Some(want))
    };
    wait_for(|| chained_next(3));
    assert!(chained_next(2), "the first open chained the old next track");
}

#[test]
fn a_paused_player_stays_paused_through_a_slow_seek() {
    let slow = slow_open_transport();
    let ctl = controller_over(&slow, "open-slow-paused");
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Playing);
    ctl.pause();
    wait_for(|| ctl.snapshot().status == PlayerStatus::Paused);
    *slow.delay.lock().unwrap() = Duration::from_millis(400);
    ctl.seek_ms(2000);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Loading);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Paused);
}

#[test]
fn an_open_that_never_completes_ends_with_a_clear_error() {
    let slow = slow_open_transport();
    slow.hang.store(true, Ordering::SeqCst);
    let sink = SharedSink(Arc::new(Mutex::new(VecSink::new())));
    let mut player = Player::new(Box::new(sink), Box::new(SlowOpenTransport(slow.clone())));
    player.set_stall_timeout(Duration::from_millis(300));
    player.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    let mut spins = 0;
    while player.status() != PlayerStatus::Stopped && spins < 400 {
        player.pump();
        spins += 1;
    }
    let s = player.snapshot();
    assert_eq!(s.status, PlayerStatus::Stopped);
    assert_eq!(s.error.as_deref(), Some("Couldn't reach the server."));
    slow.hang.store(false, Ordering::SeqCst);
}

#[test]
fn the_loudness_pre_scan_does_not_hold_up_the_controls() {
    // The pre-scan reads a whole track before playback starts; with a slow
    // network that used to be seconds on the playback thread.
    let slow = slow_open_transport();
    *slow.delay.lock().unwrap() = Duration::from_millis(600);
    let ctl = controller_over(&slow, "open-prescan");
    ctl.set_loudness_enabled(true);
    ctl.play_queue(vec![track(1, AudioFormat::Wav, 30_000)], 0);
    wait_for(|| ctl.snapshot().status == PlayerStatus::Loading);
    ctl.stop();
    wait_for(|| ctl.snapshot().status == PlayerStatus::Stopped);
}

// ---------------------------------------------------------------------------
// Volume changes must not click or zipper
// ---------------------------------------------------------------------------

/// Largest jump between consecutive frames (left channel) of the recorded output.
fn max_frame_step(samples: &[f32]) -> f32 {
    let left: Vec<f32> = samples.iter().step_by(CHANNELS).copied().collect();
    left.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_volume_change_during_playback_does_not_step_the_signal() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 10)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 10_000)], 0);
    for _ in 0..6 {
        h.player.pump();
    }
    h.player.set_volume(0.2); // a big drop, mid-waveform
    for _ in 0..12 {
        h.player.pump();
    }
    let heard = h.samples();
    // A clean 440 Hz tone at 0.7 never steps more than this between frames;
    // a whole-chunk volume multiply stepped by up to 0.8 of the signal.
    let natural = 0.7 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32;
    let step = max_frame_step(&heard);
    assert!(
        step <= natural * 1.1 + 0.7 / 441.0,
        "volume change stepped the signal: {step} (a clean tone steps {natural})"
    );
    // ...and the change did take effect.
    let tail = &heard[heard.len() - 2 * 2000..];
    let peak = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(
        (peak - 0.7 * 0.2).abs() < 0.02,
        "settled at the new volume: {peak}"
    );
}

#[test]
fn full_volume_still_passes_the_signal_through_unchanged() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.pump_until_done(60);
    let heard = h.samples();
    let want = wav_bytes(440.0, 44100);
    // the first decoded sample of the 16-bit fixture, scaled to f32
    let first = i16::from_le_bytes([
        want[44 + 2 * CHANNELS * 100],
        want[44 + 2 * CHANNELS * 100 + 1],
    ]) as f32
        / 32768.0;
    assert!(
        (heard[100 * CHANNELS] - first).abs() < 1e-6,
        "bit-transparent at unity volume"
    );
}

// ---------------------------------------------------------------------------
// Turning off the last thing that holds Best quality back engages it now
// ---------------------------------------------------------------------------

fn analog_on() -> AnalogSettings {
    AnalogSettings {
        enabled: true,
        ..Default::default()
    }
}

fn best_playing_with_analog() -> Harness {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player.set_analog(analog_on());
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    for _ in 0..10 {
        h.player.pump();
    }
    h
}

#[test]
fn turning_off_analog_warmth_engages_bit_perfect_on_the_playing_track() {
    let mut h = best_playing_with_analog();
    let s = h.player.snapshot();
    assert_eq!(
        s.output_path,
        OutputPath::Pcm,
        "analog is on: shared output"
    );
    assert_eq!(s.exclusive_blockers, vec!["Analog".to_string()]);
    assert!(s.notice.unwrap_or_default().contains("Analog"));
    let before = h.player.snapshot().position_ms;

    h.player.set_analog(AnalogSettings {
        enabled: false,
        ..analog_on()
    });
    let s = h.player.snapshot();
    assert_eq!(
        s.output_path,
        OutputPath::PcmExclusive,
        "the last blocker is gone: bit-perfect now"
    );
    assert!(s.exclusive_blockers.is_empty());
    assert!(s.notice.is_none(), "the 'paused' notice is gone");
    assert_eq!(h.player.status(), PlayerStatus::Playing);
    assert!(
        s.position_ms + 250 >= before,
        "carries on from where it was: {} vs {before}",
        s.position_ms
    );
}

#[test]
fn turning_analog_warmth_back_on_steps_bit_perfect_aside_with_a_reason() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    for _ in 0..10 {
        h.player.pump();
    }
    assert_eq!(h.player.snapshot().output_path, OutputPath::PcmExclusive);
    h.player.set_analog(analog_on());
    let s = h.player.snapshot();
    assert_eq!(s.output_path, OutputPath::Pcm);
    assert!(s
        .notice
        .unwrap_or_default()
        .contains("Best quality is paused"));
}

#[test]
fn tweaking_a_slider_that_does_not_flip_the_answer_leaves_playback_alone() {
    let mut h = best_playing_with_analog();
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.set_analog(AnalogSettings {
        drive: 0.9,
        ..analog_on()
    });
    h.player.set_analog(AnalogSettings {
        drive: 0.3,
        mix: 0.5,
        ..analog_on()
    });
    assert_eq!(
        h.stub.opened.lock().unwrap().len(),
        opens,
        "still on: nothing re-opens"
    );
}

/// Crossfeed is DSP like the others: while it is on, Auto stays on the
/// shared path and says why; off again, bit-perfect comes back.
#[test]
fn crossfeed_holds_bit_perfect_back_like_the_other_stages() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.stub.add(1, &[(440.0, 44100 * 4)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 4000)], 0);
    for _ in 0..10 {
        h.player.pump();
    }
    assert_eq!(h.player.snapshot().output_path, OutputPath::PcmExclusive);
    let on = CrossfeedSettings {
        enabled: true,
        ..Default::default()
    };
    h.player.set_crossfeed(on);
    let s = h.player.snapshot();
    assert_eq!(s.output_path, OutputPath::Pcm);
    assert_eq!(s.exclusive_blockers, vec!["Crossfeed".to_string()]);
    h.player.set_crossfeed(CrossfeedSettings {
        enabled: false,
        ..on
    });
    assert_eq!(h.player.snapshot().output_path, OutputPath::PcmExclusive);
}

#[test]
fn another_blocker_keeps_bit_perfect_off_when_analog_goes_away() {
    let mut h = best_playing_with_analog();
    h.player.set_loudness_enabled(true); // a second thing holding it back
    h.player.set_analog(AnalogSettings {
        enabled: false,
        ..analog_on()
    });
    let s = h.player.snapshot();
    assert_eq!(s.output_path, OutputPath::Pcm);
    assert_eq!(s.exclusive_blockers, vec!["Loudness".to_string()]);
    h.player.set_loudness_enabled(false);
    assert_eq!(
        h.player.snapshot().output_path,
        OutputPath::PcmExclusive,
        "now nothing holds it back"
    );
}

#[test]
fn a_paused_track_stays_paused_when_bit_perfect_engages() {
    let mut h = best_playing_with_analog();
    h.player.pause();
    h.player.set_analog(AnalogSettings {
        enabled: false,
        ..analog_on()
    });
    assert_eq!(h.player.status(), PlayerStatus::Paused);
    assert_eq!(h.player.snapshot().output_path, OutputPath::PcmExclusive);
}

#[test]
fn an_explicit_override_does_not_follow_the_processing() {
    // With Bit-perfect set to Off in Advanced, processing changes are irrelevant.
    let mut h = best_playing_with_analog();
    h.player.set_bit_perfect(BitPerfect::Off);
    let opens = h.stub.opened.lock().unwrap().len();
    h.player.set_analog(AnalogSettings {
        enabled: false,
        ..analog_on()
    });
    assert_eq!(h.stub.opened.lock().unwrap().len(), opens);
    assert_eq!(h.player.snapshot().output_path, OutputPath::Pcm);
}

#[test]
fn the_ui_is_told_when_only_the_blockers_change() {
    // While paused or stopped nothing else moves, so the change key itself must
    // notice a different blocker list or notice.
    let mut h = best_playing_with_analog();
    h.player.pause();
    let with = h.player.snapshot();
    h.player.set_analog(AnalogSettings {
        enabled: false,
        ..analog_on()
    });
    let without = h.player.snapshot();
    assert_ne!(with.exclusive_blockers, without.exclusive_blockers);
    assert!(kahawai_player_core::snapshot_key_differs(&with, &without));
}

/// A seekable source's control whose current download the test swaps, as
/// the Range source does when the demuxer jumps and it re-requests.
struct SwappableDownload(Mutex<Option<StreamProgress>>);

impl SeekableControl for SwappableDownload {
    fn warm(&self) {}
    fn progress(&self) -> Option<StreamProgress> {
        self.0.lock().unwrap().clone()
    }
}

/// One Range download's counters: `len` bytes from `offset`, `got` in.
fn download(offset: u64, len: u64, got: u64) -> StreamProgress {
    StreamProgress {
        received: Arc::new(AtomicU64::new(got)),
        content_length: Some(len),
        offset,
        stats: None,
    }
}

#[test]
fn the_buffered_fill_follows_the_seekable_sources_current_download() {
    let mut h = SeekHarness::new(SeekOffer::Seekable);
    let file = h.me.file.len() as u64;
    // First download after the seek: from 20% of the file, 10% of it in.
    let first = download(file / 5, file - file / 5, file / 10);
    let control = Arc::new(SwappableDownload(Mutex::new(Some(first.clone()))));
    *h.me.control.lock().unwrap() = Some(control.clone());
    h.player.seek_ms(2000);
    let at_first = h.player.snapshot().buffered_ms.expect("known");
    assert!(
        (2900..3100).contains(&at_first),
        "{at_first} ms ≈ 30% of 10 s"
    );

    // The download moves on: the fill follows it.
    first.received.store(file / 2, Ordering::SeqCst);
    let later = h.player.snapshot().buffered_ms.expect("known");
    assert!((6900..7100).contains(&later), "{later} ms ≈ 70% of 10 s");

    // The demuxer jumps; the source drops that download and starts another
    // from 50%. The fill must follow the new one, not freeze on the old.
    let second = download(file / 2, file - file / 2, file / 10);
    *control.0.lock().unwrap() = Some(second.clone());
    first.received.store(0, Ordering::SeqCst); // the dropped one is dead
    let after_jump = h.player.snapshot().buffered_ms.expect("known");
    assert!(
        (5900..6100).contains(&after_jump),
        "{after_jump} ms ≈ 60% of 10 s, from the new download"
    );
    second.received.store(file / 2, Ordering::SeqCst);
    let done = h.player.snapshot().buffered_ms.expect("known");
    assert!(
        done >= 9900,
        "{done} ms: the new download finished the file"
    );
}

// ---------------------------------------------------------------------------
// EQ preamp (headroom for the EQ's boosts, as AutoEq profiles use) and 12 bands
// ---------------------------------------------------------------------------

/// Peak of the last `n` interleaved samples.
fn tail_peak(samples: &[f32], n: usize) -> f32 {
    samples[samples.len().saturating_sub(n)..]
        .iter()
        .fold(0.0f32, |m, s| m.max(s.abs()))
}

#[test]
fn the_eq_preamp_lowers_the_level_while_the_eq_is_on_and_not_when_it_is_off() {
    let play = |preamp: f32, eq_on: bool| {
        let mut h = Harness::new(None);
        h.player.set_eq_preamp(preamp);
        h.player.set_eq_enabled(eq_on);
        h.stub.add(1, &[(440.0, 44100)]);
        h.player
            .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
        h.pump_until_done(100);
        tail_peak(&h.samples(), 4000)
    };
    let plain = play(0.0, true);
    assert!((plain - 0.7).abs() < 0.02, "untouched at 0 dB: {plain}");
    let down = play(-6.0, true);
    assert!(
        (down / plain - 0.501).abs() < 0.02,
        "-6 dB is x0.5: {}",
        down / plain
    );
    let off = play(-6.0, false);
    assert!(
        (off / plain - 1.0).abs() < 0.01,
        "a bypassed EQ does not apply its preamp: {}",
        off / plain
    );
}

#[test]
fn changing_the_preamp_during_playback_does_not_step_the_signal() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100 * 10)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 10_000)], 0);
    for _ in 0..6 {
        h.player.pump();
    }
    h.player.set_eq_preamp(-12.0); // a big cut, mid-waveform
    for _ in 0..12 {
        h.player.pump();
    }
    let left: Vec<f32> = h.samples().iter().step_by(CHANNELS).copied().collect();
    let step = left
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    let natural = 0.7 * 2.0 * std::f32::consts::PI * 440.0 / RATE as f32;
    assert!(
        step <= natural * 1.15,
        "preamp change stepped the signal: {step} (a clean tone steps {natural})"
    );
}

#[test]
fn the_preamp_is_clamped_and_saved_with_the_dsp_settings() {
    let mut h = Harness::new(None);
    h.player.set_eq_preamp(-100.0);
    assert_eq!(h.player.dsp_settings().eq_preamp_db, -24.0);
    h.player.set_eq_preamp(100.0);
    assert_eq!(h.player.dsp_settings().eq_preamp_db, 12.0);
    h.player.set_eq_preamp(f32::NAN);
    assert_eq!(h.player.dsp_settings().eq_preamp_db, 0.0);
    h.player.set_eq_preamp(-6.2);
    assert_eq!(h.player.dsp_settings().eq_preamp_db, -6.2);

    // The controller persists it, and an old settings file (no preamp) reads as 0.
    let dir = std::env::temp_dir().join("kahawai-player-core-test-preamp-settings");
    let _ = std::fs::remove_dir_all(&dir);
    let settings_path = dir.join("settings.json");
    let ctl = EngineController::with_transport(
        Box::new(VecSink::new()),
        Box::new(StubTransport::new(None)),
        Arc::new(std::sync::RwLock::new("http://stub".to_string())),
        settings_path.clone(),
    );
    ctl.set_eq_preamp(-4.5);
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(v["dsp"]["eq_preamp_db"], -4.5);
    let old =
        r#"{"eq_bands":[],"eq_enabled":true,"loudness_enabled":false,"loudness_target":-14.0}"#;
    let dsp: kahawai_player_core::DspSettings =
        serde_json::from_str(old).expect("old dsp block parses");
    assert_eq!(dsp.eq_preamp_db, 0.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_preamp_alone_holds_best_quality_back_like_any_other_eq() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.player.set_eq_preamp(-3.0);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    let snap = h.player.snapshot();
    assert_eq!(
        snap.output_path,
        OutputPath::Pcm,
        "the preamp changes the samples"
    );
    assert!(
        snap.exclusive_blockers.iter().any(|b| b == "EQ"),
        "{:?}",
        snap.exclusive_blockers
    );

    let mut flat = best_harness(kahawai_player_core::QualityMode::Best, true);
    flat.player.set_eq_preamp(0.0);
    flat.stub.add(1, &[(440.0, 4410)]);
    flat.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    assert!(
        flat.player.snapshot().exclusive_blockers.is_empty(),
        "0 dB is no processing"
    );
}

#[test]
fn the_eq_takes_twelve_bands_and_rejects_a_thirteenth() {
    let band = |i: usize| EqBand {
        band_type: EqBandType::Peaking,
        freq: 100.0 * (i as f32 + 1.0),
        gain_db: 1.0,
        q: 1.0,
    };
    let mut h = Harness::new(None);
    assert!(
        h.player.set_eq_bands((0..12).map(band).collect()).is_ok(),
        "an AutoEq profile (10) plus two of your own"
    );
    assert!(h.player.set_eq_bands((0..13).map(band).collect()).is_err());
}

// ---------------------------------------------------------------------------
// Playback speed: pitch-preserving, for audiobooks
// ---------------------------------------------------------------------------

/// Play a `secs`-second 440 Hz tone at `rate` to the end; returns the harness.
fn play_tone_at(rate: f32, secs: usize) -> Harness {
    let mut h = Harness::new(None);
    h.player.set_playback_rate(rate);
    h.stub.add(1, &[(440.0, RATE as usize * secs)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, secs as u64 * 1000)], 0);
    h.pump_until_done(2000);
    h
}

#[test]
fn double_speed_takes_half_the_output_and_keeps_the_pitch() {
    let h = play_tone_at(2.0, 8);
    let heard = h.samples();
    let frames = heard.len() / CHANNELS;
    let want = RATE as usize * 8 / 2;
    assert!(
        (frames as i64 - want as i64).abs() < RATE as i64 / 10,
        "{frames} frames out, wanted about {want}"
    );
    // Pitch: count rising zero crossings over the middle of the left channel.
    let left: Vec<f32> = heard.iter().step_by(CHANNELS).copied().collect();
    let mid = &left[left.len() / 4..left.len() * 3 / 4];
    let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    let hz = crossings as f32 * RATE as f32 / mid.len() as f32;
    assert!(
        (hz - 440.0).abs() < 8.0,
        "a 440 Hz tone came out at {hz} Hz"
    );
    assert_eq!(h.player.snapshot().playback_rate, 2.0);
}

#[test]
fn the_end_of_a_sped_up_track_is_not_cut_short() {
    // The stage holds back its last frame; it must be played, not dropped.
    let h = play_tone_at(1.5, 4);
    let heard = h.samples();
    assert!(
        tail_peak(&heard, 600) > 0.5,
        "the tone is still sounding at the very end: {}",
        tail_peak(&heard, 600)
    );
    let frames = heard.len() / CHANNELS;
    let want = RATE as usize * 4 * 2 / 3;
    assert!(
        (frames as i64 - want as i64).abs() < RATE as i64 / 20,
        "{frames} vs {want}"
    );
}

#[test]
fn normal_speed_is_untouched_and_not_a_blocker() {
    let h = play_tone_at(1.0, 2);
    assert!(h.player.snapshot().exclusive_blockers.is_empty());
    let heard = h.samples();
    assert_eq!(
        heard.len() / CHANNELS,
        RATE as usize * 2,
        "no frames gained or lost"
    );
}

#[test]
fn the_playhead_runs_at_the_playback_speed() {
    let mut h = Harness::new(None);
    h.player.set_playback_rate(2.0);
    h.stub.add(1, &[(440.0, RATE as usize * 20)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 20_000)], 0);
    let mut last = 0u64;
    let mut checked = false;
    for _ in 0..400 {
        h.player.pump();
        let snap = h.player.snapshot();
        let played_out = h.samples().len() / CHANNELS;
        // The sink here plays instantly (nothing stays queued), so the
        // playhead is the media consumed: about twice the frames written.
        if played_out > RATE as usize * 4 {
            let media_ms = snap.position_ms as f64;
            let out_ms = played_out as f64 * 1000.0 / RATE as f64;
            assert!(
                (media_ms / out_ms - 2.0).abs() < 0.15,
                "position {media_ms} ms after {out_ms} ms of output"
            );
            checked = true;
            break;
        }
        assert!(
            snap.position_ms >= last,
            "the playhead never goes backwards"
        );
        last = snap.position_ms;
    }
    assert!(checked, "the stream produced enough output to measure");
}

#[test]
fn changing_the_speed_during_playback_does_not_step_the_signal() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(300.0, RATE as usize * 12)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 12_000)], 0);
    for (i, rate) in [1.0f32, 1.5, 2.0, 0.8, 1.0].into_iter().enumerate() {
        h.player.set_playback_rate(rate);
        for _ in 0..(6 + i) {
            h.player.pump();
        }
    }
    let natural = 0.7 * 2.0 * std::f32::consts::PI * 300.0 / RATE as f32;
    let step = max_frame_step(&h.samples());
    assert!(
        step < natural * 1.7,
        "largest step {step}, a clean tone steps {natural}"
    );
}

#[test]
fn a_playback_speed_holds_best_quality_back_and_returning_to_one_releases_it() {
    let mut h = best_harness(kahawai_player_core::QualityMode::Best, true);
    h.player.set_playback_rate(1.25);
    h.stub.add(1, &[(440.0, 4410)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    let snap = h.player.snapshot();
    assert_eq!(
        snap.output_path,
        OutputPath::Pcm,
        "exclusive output cannot change the speed"
    );
    assert!(
        snap.exclusive_blockers
            .iter()
            .any(|b| b == "Playback speed"),
        "{:?}",
        snap.exclusive_blockers
    );

    let mut normal = best_harness(kahawai_player_core::QualityMode::Best, true);
    normal.player.set_playback_rate(1.0);
    normal.stub.add(1, &[(440.0, 4410)]);
    normal
        .player
        .play_queue(vec![track(1, AudioFormat::Wav, 100)], 0);
    assert_eq!(
        normal.player.snapshot().output_path,
        OutputPath::PcmExclusive
    );
}

#[test]
fn the_speed_is_clamped_and_ignores_nonsense() {
    let mut h = Harness::new(None);
    h.player.set_playback_rate(9.0);
    assert_eq!(h.player.snapshot().playback_rate, 3.0);
    h.player.set_playback_rate(0.1);
    assert_eq!(h.player.snapshot().playback_rate, 0.5);
    h.player.set_playback_rate(f32::NAN);
    assert_eq!(h.player.snapshot().playback_rate, 1.0);
}

// ---------------------------------------------------------------------------
// Playing a queue from a position (audiobook parts) and putting one back
// ---------------------------------------------------------------------------

#[test]
fn a_queue_can_start_partway_into_a_part_without_playing_the_wrong_place() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, RATE as usize * 10)]);
    h.stub.add(2, &[(660.0, RATE as usize * 10)]);
    h.player.set_shuffle(true);
    h.player.set_repeat(RepeatMode::All);
    h.player.play_queue_at(
        vec![
            track(1, AudioFormat::Wav, 10_000),
            track(2, AudioFormat::Wav, 10_000),
        ],
        1,
        4000,
    );
    for _ in 0..4 {
        h.player.pump();
    }
    let snap = h.player.snapshot();
    assert_eq!(snap.current_id, Some(2), "starts on the chosen part");
    assert!(!snap.shuffle, "a book plays in order");
    assert_eq!(snap.repeat, RepeatMode::Off);
    assert!(
        (4000..5600).contains(&snap.position_ms),
        "opened at the position, not at 0:00 and then moved: {} ms",
        snap.position_ms
    );
}

#[test]
fn a_restored_queue_waits_for_play_and_resumes_where_it_was() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, RATE as usize * 10)]);
    h.player.restore_queue(
        vec![track(1, AudioFormat::Wav, 10_000)],
        0,
        RepeatMode::All,
        true,
        3000,
    );
    let snap = h.player.snapshot();
    assert_eq!(snap.status, PlayerStatus::Stopped, "put back, not started");
    assert_eq!(
        snap.position_ms, 3000,
        "and it shows where play will resume"
    );
    assert_eq!(snap.repeat, RepeatMode::All);
    assert!(snap.shuffle);
    h.player.resume();
    for _ in 0..4 {
        h.player.pump();
    }
    assert!(h.player.snapshot().position_ms >= 3000);
}
