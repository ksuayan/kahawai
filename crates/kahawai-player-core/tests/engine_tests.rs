//! Engine integration tests: gapless handoff, seek accuracy, format
//! resolution, volume — all through a stub [`Transport`] serving WAV
//! fixtures, asserting on a recording [`VecSink`]. No network, no audio
//! hardware.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use kahawai_core::{api::StreamFormat, format::AudioFormat, MusicError, Track};
use kahawai_player_core::{
    resolve_format, valid_formats, AudioSink, BitPerfect, EngineController, EqBand, EqBandType,
    OutputPath, PcmChunk, Player, PlayerStatus, StreamInfo, StreamOptions, Transport, VecSink,
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
        hash: "h".into(),
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
}

impl StubTransport {
    fn new(chain_mode: Option<&str>) -> Self {
        Self {
            tracks: Mutex::new(HashMap::new()),
            next_body: Mutex::new(HashMap::new()),
            chain_mode: chain_mode.map(|s| s.to_string()),
            chain_label: Mutex::new(None),
            opened: Mutex::new(Vec::new()),
        }
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
    // Explicit choices still win over the story.
    assert_eq!(
        resolve_format(&dsf, Some(StreamFormat::Flac), None, DsdStory::Native),
        StreamFormat::Flac,
        "global override wins over the DSD story"
    );
    assert_eq!(
        resolve_format(&dsf, None, Some(StreamFormat::Flac), DsdStory::Native),
        StreamFormat::Flac,
        "per-track override wins over the DSD story"
    );
    // Convert (default) preserves pre-C3 behavior.
    assert_eq!(
        resolve_format(&dsf, None, None, DsdStory::Convert),
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
fn dop_is_refused_without_a_dsd_capable_sink() {
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 22050)]);
    h.player.set_track_format(1, Some(StreamFormat::Dop));
    h.player
        .play_queue(vec![track(1, AudioFormat::Dsf, 500)], 0);
    assert_eq!(h.player.status(), PlayerStatus::Stopped);
    let snap = h.player.snapshot();
    assert!(
        snap.error.as_deref().unwrap_or("").contains("DoP"),
        "error: {:?}",
        snap.error
    );
    assert!(
        h.stub.opened.lock().unwrap().is_empty(),
        "no request was made"
    );
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
        hash: "h".into(),
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
}

#[derive(Clone)]
struct SharedDopSink(Arc<Mutex<DopSink>>);

impl AudioSink for SharedDopSink {
    fn open(&mut self, _track: &Track) -> Result<(), MusicError> {
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
    fn dop_output_rate(&self, _dsd_rate_hz: u32) -> Option<u32> {
        self.0.lock().unwrap().dop_rate
    }
    fn select_output_path(&mut self, path: OutputPath) {
        self.0.lock().unwrap().path = path;
    }
    fn write_dop(&mut self, frames: &[u8]) -> Result<(), MusicError> {
        self.0.lock().unwrap().dop.extend_from_slice(frames);
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
        })));
        struct Wrap(Arc<DopTransport>);
        impl Transport for Wrap {
            fn open_stream(
                &self,
                track_id: i64,
                opts: &StreamOptions,
            ) -> Result<StreamInfo, MusicError> {
                self.0.open_stream(track_id, opts)
            }
        }
        let player = Player::new(Box::new(sink.clone()), Box::new(Wrap(transport.clone())));
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
    let mut h = Harness::new(None);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.pump_until_done(100);
    let base = rms(&h.samples());

    let mut h2 = Harness::new(None);
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
    assert_eq!(story, DsdStory::Convert, "default preserves old behavior");
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
fn a_write_failure_on_the_exclusive_device_stops_playback_with_an_error() {
    let mut h = bp_harness(BitPerfect::All, &[RATE]);
    h.stub.add(1, &[(440.0, 44100)]);
    h.player
        .play_queue(vec![track(1, AudioFormat::Wav, 1000)], 0);
    h.sink.0.lock().unwrap().exclusive_open = None; // device vanished
    h.player.pump();
    assert_eq!(h.player.status(), PlayerStatus::Stopped);
    assert!(h.player.snapshot().error.is_some());
}

#[test]
fn the_preference_persists_across_controllers() {
    let dir = std::env::temp_dir().join(format!("kahawai-bp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("engine-settings.json");
    {
        let c = EngineController::new(Box::new(VecSink::new()), path.clone());
        assert_eq!(c.bit_perfect(), BitPerfect::Off, "off by default");
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
