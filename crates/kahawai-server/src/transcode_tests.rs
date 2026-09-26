// ---------------------------------------------------------------------------
// Tests (S5a, S6, S12-transcode, S13)
// ---------------------------------------------------------------------------

use super::*;
use std::io::Cursor;
use std::sync::OnceLock;

/// 16-bit PCM WAV writer: `frames` sine frames at `freq` Hz.
fn write_wav(path: &Path, sample_rate: u32, channels: usize, frames: usize, freq: f32) {
    let mut v = Vec::new();
    let data_len = (frames * channels * 2) as u32;
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&(channels as u16).to_le_bytes());
    v.extend_from_slice(&sample_rate.to_le_bytes());
    v.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&((channels * 2) as u16).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for f in 0..frames {
        let t = f as f32 / sample_rate as f32;
        let s = (2.0 * std::f32::consts::PI * freq * t).sin();
        let q = (s * 32767.0).round() as i16;
        for _ in 0..channels {
            v.extend_from_slice(&q.to_le_bytes());
        }
    }
    std::fs::write(path, v).unwrap();
}

/// Minimal stereo DSD64 DSF: `blocks` block-groups of `block_len` bytes
/// per channel, filled with `fill` (0xFE ≈ +0.75 DC).
fn write_dsf(path: &Path, blocks: usize, block_len: usize, fill: u8) {
    fn w32le(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn w64le(v: &mut Vec<u8>, x: u64) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    let mut v = Vec::new();
    v.extend_from_slice(b"DSD ");
    w64le(&mut v, 28);
    let size_pos = v.len();
    w64le(&mut v, 0);
    let ptr_pos = v.len();
    w64le(&mut v, 0);
    v.extend_from_slice(b"fmt ");
    w64le(&mut v, 52); // 12-byte header + 40 bytes of fields (Sony spec)
    w32le(&mut v, 1); // version
    w32le(&mut v, 0); // uncompressed
    w32le(&mut v, 2); // stereo
    w32le(&mut v, 2); // channels
    w32le(&mut v, 2_822_400);
    w32le(&mut v, 1); // bits per sample
    w64le(&mut v, (blocks * block_len * 8) as u64);
    w32le(&mut v, block_len as u32);
    w32le(&mut v, 0); // reserved
    let data_off = v.len();
    v.extend_from_slice(b"data");
    w64le(&mut v, (12 + blocks * 2 * block_len) as u64);
    for _ in 0..blocks {
        v.extend(std::iter::repeat_n(fill, block_len));
        v.extend(std::iter::repeat_n(fill, block_len));
    }
    let total = v.len() as u64;
    v[size_pos..size_pos + 8].copy_from_slice(&total.to_le_bytes());
    v[ptr_pos..ptr_pos + 8].copy_from_slice(&(data_off as u64).to_le_bytes());
    std::fs::write(path, v).unwrap();
}

fn ffmpeg_available() -> bool {
    static AVAIL: OnceLock<bool> = OnceLock::new();
    *AVAIL.get_or_init(|| {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// ffmpeg-generated fixture: 1 s sine → `codec`.
fn ffmpeg_sine(path: &Path, codec: &str, bitrate: &str) {
    assert!(
        ffmpeg_available(),
        "ffmpeg binary is required for this fixture"
    );
    let st = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1:sample_rate=44100",
            "-ac",
            "2",
            "-c:a",
            codec,
            "-b:a",
            bitrate,
            path.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg binary is required for this fixture");
    assert!(st.success(), "ffmpeg {codec} encode failed");
}

/// Run a whole transcode on the calling thread's behalf: the producer
/// runs on a worker thread (the mpsc channel is bounded, so the
/// consumer must drain concurrently).
fn transcode_to_bytes(plan: &TranscodePlan) -> (Vec<u8>, String) {
    let prepared = PreparedTranscode::setup(plan).expect("transcode setup");
    let chain = prepared.chain.clone();
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, MusicError>>(32);
    let worker = std::thread::spawn(move || prepared.run(&tx));
    let mut out = Vec::new();
    while let Some(item) = rx.blocking_recv() {
        out.extend_from_slice(&item.expect("transcode chunk"));
    }
    worker.join().expect("producer thread");
    (out, chain)
}

/// Decode a FLAC byte stream with symphonia → (interleaved f32, rate, channels).
fn decode_flac_bytes(bytes: &[u8]) -> (Vec<f32>, u32, usize) {
    use symphonia::core::codecs::registry::CodecRegistry;
    let cursor = Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("probe transcoded flac");
    let track = format.default_track(TrackType::Audio).expect("audio track");
    let track_id = track.id;
    let params = match track.codec_params.as_ref() {
        Some(symphonia::core::codecs::CodecParameters::Audio(p)) => p.clone(),
        _ => panic!("no audio params"),
    };
    let rate = params.sample_rate.unwrap();
    let channels = params.channels.as_ref().unwrap().count();
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    let mut decoder = registry
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .expect("flac decoder");
    let mut pcm = Vec::new();
    while let Some(packet) = format.next_packet().expect("packet") {
        if packet.track_id != track_id {
            continue;
        }
        append_audio_ref(&mut pcm, decoder.decode(&packet).expect("decode"));
    }
    (pcm, rate, channels)
}

fn default_ladder() -> Vec<StreamFormat> {
    vec![
        StreamFormat::Passthrough,
        StreamFormat::Flac,
        StreamFormat::Opus,
    ]
}

#[test]
fn wav_to_flac_roundtrip_is_near_lossless() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("sine.wav");
    // 0.5 s stereo 48 kHz — not a multiple of the 4096 block size, so
    // the partial-final-block path is exercised.
    write_wav(&wav, 48_000, 2, 24_000, 440.0);

    let plan = resolve_plan(
        AudioFormat::Wav,
        wav,
        Some(StreamFormat::Flac),
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("not passthrough");
    let (bytes, chain) = transcode_to_bytes(&plan);
    assert_eq!(&bytes[..4], b"fLaC");
    assert!(chain.contains("flac"), "chain: {chain}");

    let (pcm, rate, channels) = decode_flac_bytes(&bytes);
    assert_eq!(rate, 48_000);
    assert_eq!(channels, 2);
    // 16-bit source → 24-bit FLAC: the only error is the 24-bit
    // quantization step (2^-24), plus float rounding. Rebuild the
    // exact 16-bit source samples the writer produced.
    assert_eq!(pcm.len(), 24_000 * 2);
    let mut max_diff: f32 = 0.0;
    for (i, &s) in pcm.iter().enumerate() {
        let f = (i / 2) as f32;
        let t = f / 48_000.0;
        let q = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 32767.0;
        let expect = q.round() / 32768.0;
        max_diff = max_diff.max((s - expect).abs());
    }
    assert!(max_diff < 1e-6, "max round-trip deviation {max_diff}");
}

#[test]
fn wav_to_flac_seek_ms_is_sample_exact() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("sine.wav");
    write_wav(&wav, 48_000, 2, 96_000, 440.0); // 2 s

    let mk = |seek_ms| {
        resolve_plan(
            AudioFormat::Wav,
            wav.clone(),
            Some(StreamFormat::Flac),
            &default_ladder(),
            DsdStory::Pcm,
            seek_ms,
        )
        .expect("plan")
        .expect("not passthrough")
    };
    let (full, _) = transcode_to_bytes(&mk(None));
    let (seeked, _) = transcode_to_bytes(&mk(Some(500)));

    let (full_pcm, _, _) = decode_flac_bytes(&full);
    let (seek_pcm, _, _) = decode_flac_bytes(&seeked);
    // 500 ms @ 48 kHz = frame 24_000.
    let start = 24_000 * 2;
    assert!(full_pcm.len() > start + 2048);
    // Sample-exact: the seeked stream must equal the full stream from
    // the seek point (24-bit quantization is deterministic).
    let mut max_diff: f32 = 0.0;
    for i in 0..2048 {
        max_diff = max_diff.max((seek_pcm[i] - full_pcm[start + i]).abs());
    }
    assert_eq!(max_diff, 0.0, "seek granularity is sample-exact");
}

#[test]
fn dsf_to_flac_decimates_to_88200() {
    let dir = tempfile::tempdir().unwrap();
    let dsf = dir.path().join("tone.dsf");
    // 0xFE = 7/8 ones → +0.75 DC after the FIR settles.
    write_dsf(&dsf, 48, 4096, 0xFE);

    let plan = resolve_plan(
        AudioFormat::Dsf,
        dsf,
        None, // ladder decides → FLAC for DSD
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("DSD must not passthrough");
    assert_eq!(plan.target, StreamFormat::Flac);
    let (bytes, chain) = transcode_to_bytes(&plan);
    assert!(chain.starts_with("dsf64"), "chain: {chain}");

    let (pcm, rate, channels) = decode_flac_bytes(&bytes);
    assert_eq!(rate, 88_200);
    assert_eq!(channels, 2);
    // Skip the FIR warm-up transient; the steady state is +0.75 DC.
    let steady: Vec<f32> = pcm
        .chunks(2)
        .skip(8_000)
        .map(|f| (f[0] + f[1]) / 2.0)
        .collect();
    assert!(!steady.is_empty());
    let mean: f32 = steady.iter().sum::<f32>() / steady.len() as f32;
    assert!(
        (mean - 0.75).abs() < 0.05,
        "DSD DC level {mean}, want ≈0.75"
    );
}

#[test]
fn mp3_to_flac_decodes() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mp3 = dir.path().join("sine.mp3");
    ffmpeg_sine(&mp3, "libmp3lame", "128k");

    let plan = resolve_plan(
        AudioFormat::Mp3,
        mp3,
        Some(StreamFormat::Flac),
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("not passthrough");
    let (bytes, chain) = transcode_to_bytes(&plan);
    assert!(chain.contains("mp3"), "chain: {chain}");
    let (pcm, rate, channels) = decode_flac_bytes(&bytes);
    assert_eq!(rate, 44_100);
    assert_eq!(channels, 2);
    // MP3 encoder delay/padding shifts the duration; allow ±0.1 s.
    let secs = pcm.len() as f32 / (rate as f32 * channels as f32);
    assert!((secs - 1.0).abs() < 0.1, "decoded {secs}s");
    let energy: f32 = pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32;
    let peak: f32 = pcm.iter().map(|s| s.abs()).fold(0.0, f32::max);
    // The lavfi sine fixture is quiet (≈ -20 dBFS); just require clearly
    // non-silent output.
    assert!(energy > 0.001, "decoded silence? energy={energy}");
    assert!(peak > 0.05, "decoded silence? peak={peak}");
}

#[test]
fn opus_source_decodes_through_adapter() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let opus = dir.path().join("sine.opus");
    ffmpeg_sine(&opus, "libopus", "96k");

    // Decode-only: SymphoniaSource must handle the opus container via
    // symphonia-adapter-libopus.
    let mut src = SymphoniaSource::open(&opus).expect("open opus");
    let spec = src.spec();
    assert_eq!(spec.sample_rate, 48_000);
    let mut pcm = vec![0.0f32; 8192 * spec.channels];
    let mut total = 0usize;
    loop {
        let n = src.fill(&mut pcm).expect("fill");
        if n == 0 {
            break;
        }
        total += n;
    }
    let secs = total as f32 / (spec.sample_rate as f32 * spec.channels as f32);
    assert!((secs - 1.0).abs() < 0.1, "decoded {secs}s");
}

#[test]
fn resolve_plan_passthrough_cases() {
    let p = PathBuf::from("/tmp/x.wav");
    // No explicit format + streamable source → passthrough.
    assert!(resolve_plan(
        AudioFormat::Wav,
        p.clone(),
        None,
        &default_ladder(),
        DsdStory::Pcm,
        None
    )
    .expect("plan")
    .is_none());
    // Explicit flac wins over the ladder.
    let plan = resolve_plan(
        AudioFormat::Wav,
        p.clone(),
        Some(StreamFormat::Flac),
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("some");
    assert_eq!(plan.target, StreamFormat::Flac);
    // FLAC → FLAC is a no-op.
    assert!(resolve_plan(
        AudioFormat::Flac,
        p.clone(),
        Some(StreamFormat::Flac),
        &default_ladder(),
        DsdStory::Pcm,
        None
    )
    .expect("plan")
    .is_none());
}

#[test]
fn resolve_plan_dsd_stories() {
    let p = PathBuf::from("/tmp/x.dsf");
    // Missing format + DSD/Pcm → FLAC.
    let plan = resolve_plan(
        AudioFormat::Dsf,
        p.clone(),
        None,
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("some");
    assert_eq!(plan.target, StreamFormat::Flac);
    // DSD/Native with no explicit format → DoP (S5b).
    let plan = resolve_plan(
        AudioFormat::Dsf,
        p.clone(),
        None,
        &default_ladder(),
        DsdStory::Native,
        None,
    )
    .expect("plan")
    .expect("some");
    assert_eq!(plan.target, StreamFormat::Dop);
    // Explicit ?format=dop → DoP under either story.
    for story in [DsdStory::Native, DsdStory::Pcm] {
        let plan = resolve_plan(
            AudioFormat::Dff,
            PathBuf::from("/tmp/x.dff"),
            Some(StreamFormat::Dop),
            &default_ladder(),
            story,
            Some(1500),
        )
        .expect("plan")
        .expect("some");
        assert_eq!(plan.target, StreamFormat::Dop);
        assert_eq!(plan.seek_ms, Some(1500));
    }
    // Explicit ?format=dop on a non-DSD source → 400-class error.
    let err = resolve_plan(
        AudioFormat::Flac,
        PathBuf::from("/tmp/x.flac"),
        Some(StreamFormat::Dop),
        &default_ladder(),
        DsdStory::Native,
        None,
    )
    .expect_err("dop on non-DSD must fail");
    assert!(
        matches!(err, MusicError::BadRequest(_)),
        "wrong error: {err:?}"
    );
    // Explicit non-DoP format under the native story is still honored:
    // ?format=flac → DSD→PCM→FLAC.
    let plan = resolve_plan(
        AudioFormat::Dsf,
        p.clone(),
        Some(StreamFormat::Flac),
        &default_ladder(),
        DsdStory::Native,
        None,
    )
    .expect("plan")
    .expect("some");
    assert_eq!(plan.target, StreamFormat::Flac);
    // ?format=passthrough on DSD keeps the existing coercion to FLAC.
    let plan = resolve_plan(
        AudioFormat::Dsf,
        p.clone(),
        Some(StreamFormat::Passthrough),
        &default_ladder(),
        DsdStory::Native,
        None,
    )
    .expect("plan")
    .expect("some");
    assert_eq!(plan.target, StreamFormat::Flac);
    // SACD ISO is never decoded inline, under either story.
    for story in [DsdStory::Pcm, DsdStory::Native] {
        let err = resolve_plan(
            AudioFormat::SacdIso,
            PathBuf::from("/tmp/x.iso"),
            None,
            &default_ladder(),
            story,
            None,
        )
        .expect_err("iso must fail");
        assert!(matches!(err, MusicError::UnsupportedFormat(_)));
    }
    // SACD ISO + explicit ?format=dop → still the offline-only error.
    let err = resolve_plan(
        AudioFormat::SacdIso,
        PathBuf::from("/tmp/x.iso"),
        Some(StreamFormat::Dop),
        &default_ladder(),
        DsdStory::Native,
        None,
    )
    .expect_err("iso+dop must fail");
    assert!(matches!(err, MusicError::UnsupportedFormat(_)));
}

#[test]
fn disabled_encoders_name_their_features() {
    let p = PathBuf::from("/tmp/x.wav");
    #[cfg(not(feature = "encode-opus"))]
    {
        let err = resolve_plan(
            AudioFormat::Wav,
            p.clone(),
            Some(StreamFormat::Opus),
            &default_ladder(),
            DsdStory::Pcm,
            None,
        )
        .expect_err("opus encoder must be disabled");
        match err {
            MusicError::FeatureDisabled { feature, .. } => assert_eq!(feature, "encode-opus"),
            other => panic!("wrong error: {other:?}"),
        }
    }
    #[cfg(not(feature = "encode-mp3"))]
    {
        let err = resolve_plan(
            AudioFormat::Wav,
            p.clone(),
            Some(StreamFormat::Mp3),
            &default_ladder(),
            DsdStory::Pcm,
            None,
        )
        .expect_err("mp3 encoder must be disabled");
        match err {
            MusicError::FeatureDisabled { feature, .. } => assert_eq!(feature, "encode-mp3"),
            other => panic!("wrong error: {other:?}"),
        }
    }
    #[cfg(feature = "encode-opus")]
    {
        let plan = resolve_plan(
            AudioFormat::Wav,
            p.clone(),
            Some(StreamFormat::Opus),
            &default_ladder(),
            DsdStory::Pcm,
            None,
        )
        .expect("plan")
        .expect("some");
        assert_eq!(plan.target, StreamFormat::Opus);
    }
    #[cfg(feature = "encode-mp3")]
    {
        let plan = resolve_plan(
            AudioFormat::Wav,
            p.clone(),
            Some(StreamFormat::Mp3),
            &default_ladder(),
            DsdStory::Pcm,
            None,
        )
        .expect("plan")
        .expect("some");
        assert_eq!(plan.target, StreamFormat::Mp3);
    }
}

#[test]
fn flac_stream_header_is_well_formed() {
    let enc = FlacStreamEncoder::new(48_000, 2).expect("encoder");
    let hdr = enc.header_bytes();
    assert_eq!(&hdr[..4], b"fLaC");
    // Metadata block header: last-block=1, type=0 (STREAMINFO), len=34.
    assert_eq!(hdr[4], 0x80);
    assert_eq!(hdr[5], 0x00);
    assert_eq!(&hdr[6..8], &[0x00, 0x22]);
    assert_eq!(hdr.len(), 4 + 4 + 34);
}
/// ffmpeg must accept the streamed FLAC end to end (external check of the
/// hand-written container bytes, beyond symphonia).
#[test]
fn ffmpeg_validates_streamed_flac() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("sine.wav");
    write_wav(&wav, 44_100, 2, 44_100, 440.0);
    let plan = resolve_plan(
        AudioFormat::Wav,
        wav,
        Some(StreamFormat::Flac),
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("not passthrough");
    let (bytes, _) = transcode_to_bytes(&plan);
    let flac = dir.path().join("out.flac");
    std::fs::write(&flac, &bytes).unwrap();
    let out = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            flac.to_str().unwrap(),
            "-f",
            "null",
            "-",
        ])
        .output()
        .expect("ffmpeg binary is required for this test");
    assert!(
        out.status.success(),
        "ffmpeg rejected streamed flac: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Opus encoder runtime check (feature `encode-opus`): the hand-written Ogg
/// muxer + OpusHead must decode in ffmpeg with the right rate/channels.
#[cfg(feature = "encode-opus")]
#[test]
fn opus_encoder_output_decodes() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("sine.wav");
    write_wav(&wav, 48_000, 2, 48_000, 440.0);
    let plan = resolve_plan(
        AudioFormat::Wav,
        wav,
        Some(StreamFormat::Opus),
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("not passthrough");
    let (bytes, chain) = transcode_to_bytes(&plan);
    assert!(chain.contains("opus"), "chain: {chain}");
    let opus = dir.path().join("out.opus");
    std::fs::write(&opus, &bytes).unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-show_entries",
            "stream=codec_name,sample_rate,channels,duration",
            "-of",
            "default=noprint_wrappers=1",
            opus.to_str().unwrap(),
        ])
        .output()
        .expect("ffprobe is required for this test");
    assert!(probe.status.success());
    let info = String::from_utf8_lossy(&probe.stdout);
    assert!(info.contains("codec_name=opus"), "ffprobe: {info}");
    assert!(info.contains("sample_rate=48000"), "ffprobe: {info}");
    assert!(info.contains("channels=2"), "ffprobe: {info}");
    // Fully decode to catch muxing errors mid-stream.
    let dec = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            opus.to_str().unwrap(),
            "-f",
            "null",
            "-",
        ])
        .output()
        .expect("ffmpeg binary is required for this test");
    assert!(
        dec.status.success(),
        "ffmpeg rejected opus stream: {}",
        String::from_utf8_lossy(&dec.stderr)
    );
}

/// Parse Ogg page headers: (header_type_flags, granule_position) per page.
#[cfg(feature = "encode-opus")]
fn ogg_page_headers(bytes: &[u8]) -> Vec<(u8, u64)> {
    let mut pages = Vec::new();
    let mut pos = 0;
    while pos + 27 <= bytes.len() {
        assert_eq!(&bytes[pos..pos + 4], b"OggS", "bad capture pattern");
        let flags = bytes[pos + 5];
        let granule = u64::from_le_bytes(bytes[pos + 6..pos + 14].try_into().unwrap());
        let nseg = bytes[pos + 26] as usize;
        assert!(pos + 27 + nseg <= bytes.len());
        let body: usize = bytes[pos + 27..pos + 27 + nseg]
            .iter()
            .map(|&s| s as usize)
            .sum();
        pages.push((flags, granule));
        pos += 27 + nseg + body;
    }
    assert_eq!(pos, bytes.len(), "trailing bytes after last Ogg page");
    pages
}

/// RFC 7845 granule semantics: page granules count decoded samples *including*
/// the pre-skip region — first audio page = pre_skip + 960, final EOS page =
/// pre_skip + real input samples.
#[cfg(feature = "encode-opus")]
#[test]
fn opus_granule_positions_include_preskip() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("sine.wav");
    let input_samples = 48_000u64; // exactly 1 s @ 48 kHz
    write_wav(&wav, 48_000, 2, input_samples as usize, 440.0);
    let plan = resolve_plan(
        AudioFormat::Wav,
        wav,
        Some(StreamFormat::Opus),
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("not passthrough");
    let (bytes, _) = transcode_to_bytes(&plan);

    let pages = ogg_page_headers(&bytes);
    assert!(
        pages.len() >= 4,
        "want head+tags+audio+eos, got {}",
        pages.len()
    );
    // BOS + tags pages carry granule 0.
    assert_eq!(pages[0].1, 0);
    assert_eq!(pages[1].1, 0);
    // OpusHead pre-skip lives at packet offset 10 (u16 LE).
    // First page body starts after its header + segment table.
    let first_body = {
        let nseg = bytes[26] as usize;
        27 + nseg
    };
    let preskip = u16::from_le_bytes(bytes[first_body + 10..first_body + 12].try_into().unwrap());
    assert!(preskip > 0, "encoder must report real lookahead");
    // First audio page granule = pre_skip + one 960-sample frame.
    assert_eq!(pages[2].1, preskip as u64 + 960, "first audio page granule");
    // Final EOS page granule = pre_skip + real input samples.
    let (flags, granule) = pages[pages.len() - 1];
    assert_eq!(flags & 0x04, 0x04, "last page must carry EOS");
    assert_eq!(
        granule,
        preskip as u64 + input_samples,
        "final granule must trim tail padding exactly"
    );
}

/// MP3 encoder runtime check (feature `encode-mp3`).
#[cfg(feature = "encode-mp3")]
#[test]
fn mp3_encoder_output_decodes() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("sine.wav");
    write_wav(&wav, 44_100, 2, 44_100, 440.0);
    let plan = resolve_plan(
        AudioFormat::Wav,
        wav,
        Some(StreamFormat::Mp3),
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("not passthrough");
    let (bytes, chain) = transcode_to_bytes(&plan);
    assert!(chain.contains("mp3"), "chain: {chain}");
    assert!(bytes.len() > 1000, "suspiciously small mp3");
    let mp3 = dir.path().join("out.mp3");
    std::fs::write(&mp3, &bytes).unwrap();
    let dec = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            mp3.to_str().unwrap(),
            "-f",
            "null",
            "-",
        ])
        .output()
        .expect("ffmpeg binary is required for this test");
    assert!(
        dec.status.success(),
        "ffmpeg rejected mp3 stream: {}",
        String::from_utf8_lossy(&dec.stderr)
    );
}

/// Minimal stereo DSD64 DFF fixture with spec-correct chunk sizes
/// (ckDataSize = data portion only, DSDIFF 1.5) and MSB-first audio bytes.
fn write_dff(path: &Path, frames: usize, fill: u8) {
    fn w32be(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn w64be(v: &mut Vec<u8>, x: u64) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    let mut v = Vec::new();
    v.extend_from_slice(b"FRM8");
    let form_pos = v.len();
    w64be(&mut v, 0);
    v.extend_from_slice(b"DSD ");
    v.extend_from_slice(b"FVER");
    w64be(&mut v, 4);
    w32be(&mut v, 0x0105_0000);
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(b"FS  ");
    w64be(&mut prop, 4);
    w32be(&mut prop, 2_822_400);
    prop.extend_from_slice(b"CHNL");
    w64be(&mut prop, 10);
    prop.extend_from_slice(&2u16.to_be_bytes());
    prop.extend_from_slice(&[0u8; 8]);
    prop.extend_from_slice(b"CMPR");
    w64be(&mut prop, 4);
    prop.extend_from_slice(b"DSD ");
    v.extend_from_slice(b"PROP");
    w64be(&mut v, prop.len() as u64);
    v.extend_from_slice(&prop);
    v.extend_from_slice(b"DSD ");
    w64be(&mut v, (frames * 2) as u64);
    for _ in 0..frames {
        v.push(fill);
        v.push(fill);
    }
    let total = v.len() as u64;
    v[form_pos..form_pos + 8].copy_from_slice(&(total - 12).to_be_bytes());
    std::fs::write(path, v).unwrap();
}

#[test]
fn dff_to_flac_decimates_to_88200() {
    let dir = tempfile::tempdir().unwrap();
    let dff = dir.path().join("tone.dff");
    // 0x7F MSB-first = 7/8 ones → +0.75 DC after the FIR settles.
    write_dff(&dff, 196_608, 0x7F);

    let plan = resolve_plan(
        AudioFormat::Dff,
        dff,
        None,
        &default_ladder(),
        DsdStory::Pcm,
        None,
    )
    .expect("plan")
    .expect("DSD must not passthrough");
    assert_eq!(plan.target, StreamFormat::Flac);
    let (bytes, chain) = transcode_to_bytes(&plan);
    assert!(chain.starts_with("dff64"), "chain: {chain}");

    let (pcm, rate, channels) = decode_flac_bytes(&bytes);
    assert_eq!(rate, 88_200);
    assert_eq!(channels, 2);
    let steady: Vec<f32> = pcm
        .chunks(2)
        .skip(8_000)
        .map(|f| (f[0] + f[1]) / 2.0)
        .collect();
    assert!(!steady.is_empty());
    let mean: f32 = steady.iter().sum::<f32>() / steady.len() as f32;
    assert!(
        (mean - 0.75).abs() < 0.05,
        "DSD DC level {mean}, want ≈0.75"
    );
}

// ---------------------------------------------------------------------------
// S8 gapless chaining tests
// ---------------------------------------------------------------------------

/// Run a gapless chain to bytes, returning (bytes, chain, mode).
fn gapless_to_bytes(plans: &[TranscodePlan]) -> (Vec<u8>, String, GaplessMode) {
    let prepared = PreparedGapless::setup(plans).expect("gapless setup");
    let chain = prepared.chain.clone();
    let mode = prepared.mode;
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, MusicError>>(32);
    let worker = std::thread::spawn(move || prepared.run(&tx));
    let mut out = Vec::new();
    while let Some(item) = rx.blocking_recv() {
        out.extend_from_slice(&item.expect("gapless chunk"));
    }
    worker.join().expect("producer thread");
    (out, chain, mode)
}

/// 16-bit WAV writer with a starting phase, for phase-continuous chains.
fn write_wav_phased(
    path: &Path,
    sample_rate: u32,
    channels: usize,
    frames: usize,
    freq: f32,
    phase0: f32,
) {
    let mut v = Vec::new();
    let data_len = (frames * channels * 2) as u32;
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&(channels as u16).to_le_bytes());
    v.extend_from_slice(&sample_rate.to_le_bytes());
    v.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&((channels * 2) as u16).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for f in 0..frames {
        let t = f as f32 / sample_rate as f32;
        let s = (phase0 + 2.0 * std::f32::consts::PI * freq * t).sin();
        let q = (s * 32767.0).round() as i16;
        for _ in 0..channels {
            v.extend_from_slice(&q.to_le_bytes());
        }
    }
    std::fs::write(path, v).unwrap();
}

/// Ideal decoded f32 samples for a phased 16-bit sine: exact through the
/// lossless FLAC round-trip (16-bit source × 24-bit FLAC is bit-exact).
fn ideal_sine(frames: usize, sample_rate: u32, freq: f32, phase0: f32) -> Vec<f32> {
    (0..frames)
        .map(|f| {
            let t = f as f32 / sample_rate as f32;
            let s = (phase0 + 2.0 * std::f32::consts::PI * freq * t).sin();
            (s * 32767.0).round() as i16 as f32 / 32768.0
        })
        .collect()
}

fn gapless_flac_plans(a: PathBuf, b: PathBuf) -> [TranscodePlan; 2] {
    [
        TranscodePlan {
            path: a,
            source_format: AudioFormat::Wav,
            target: StreamFormat::Flac,
            seek_ms: None,
        },
        TranscodePlan {
            path: b,
            source_format: AudioFormat::Wav,
            target: StreamFormat::Flac,
            seek_ms: None,
        },
    ]
}

#[test]
fn gapless_single_session_flac_has_no_boundary_dip() {
    let dir = tempfile::tempdir().unwrap();
    let rate = 44_100u32;
    let frames = 4410usize; // 0.1 s; 440 Hz → exactly 44 cycles
    let freq = 440.0f32;
    let a = dir.path().join("a.wav");
    let b = dir.path().join("b.wav");
    write_wav_phased(&a, rate, 2, frames, freq, 0.0);
    // Phase where track A ended — the chain must be phase-continuous.
    let phase_b = 2.0 * std::f32::consts::PI * freq * (frames as f32 / rate as f32);
    write_wav_phased(&b, rate, 2, frames, freq, phase_b);

    let plans = gapless_flac_plans(a, b);
    let (bytes, chain, mode) = gapless_to_bytes(&plans);
    assert_eq!(mode, GaplessMode::SingleSession);
    assert!(chain.contains("wav->flac"), "chain: {chain}");
    // One continuous FLAC stream: exactly one stream header.
    let magics = bytes.windows(4).filter(|w| *w == b"fLaC").count();
    assert_eq!(magics, 1, "single session must emit one FLAC header");

    let (pcm, dec_rate, dec_ch) = decode_flac_bytes(&bytes);
    assert_eq!(dec_rate, rate);
    assert_eq!(dec_ch, 2);
    assert_eq!(pcm.len(), 2 * frames * 2, "total samples = both tracks");

    // Every decoded sample matches the ideal phase-continuous sine —
    // no boundary artifact where the two tracks meet.
    let mut ideal = ideal_sine(frames, rate, freq, 0.0);
    ideal.extend(ideal_sine(frames, rate, freq, phase_b));
    assert_eq!(pcm.len(), ideal.len() * 2); // stereo: ideal is per-channel
    for (i, (&got, want)) in pcm
        .iter()
        .zip(ideal.iter().flat_map(|&x| [x, x]))
        .enumerate()
    {
        assert!(
            (got - want).abs() < 1e-4,
            "sample {i}: got {got}, want {want}"
        );
    }

    // Explicit energy check on ±128 frames around the boundary.
    let bnd = frames * 2; // interleaved index of the first sample of track B
    let win = &pcm[bnd - 256..bnd + 256];
    let energy: f32 = win.iter().map(|s| s * s).sum::<f32>() / win.len() as f32;
    let ideal_win: Vec<f32> = ideal
        .iter()
        .flat_map(|&x| [x, x])
        .skip(bnd - 256)
        .take(512)
        .collect();
    let ideal_energy: f32 = ideal_win.iter().map(|s| s * s).sum::<f32>() / ideal_win.len() as f32;
    assert!(
        (energy - ideal_energy).abs() / ideal_energy < 0.01,
        "boundary energy dip? got {energy}, want {ideal_energy}"
    );
}

#[test]
fn gapless_chained_flac_on_mismatched_specs() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    let b = dir.path().join("b.wav");
    write_wav(&a, 44_100, 2, 4410, 440.0);
    write_wav(&b, 48_000, 2, 4800, 440.0);

    let plans = gapless_flac_plans(a, b);
    let (bytes, chain, mode) = gapless_to_bytes(&plans);
    assert_eq!(mode, GaplessMode::Chained);
    assert!(chain.contains('+'), "chain: {chain}");
    // Two independent FLAC streams concatenated: two stream headers.
    let magics = bytes.windows(4).filter(|w| *w == b"fLaC").count();
    assert_eq!(magics, 2, "chained mode must emit two FLAC headers");

    // Each stream decodes independently to its own track.
    let mut positions: Vec<usize> = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"fLaC")
        .map(|(i, _)| i)
        .collect();
    positions.sort_unstable();
    assert_eq!(positions.len(), 2);
    let (pcm1, rate1, _) = decode_flac_bytes(&bytes[..positions[1]]);
    assert_eq!(rate1, 44_100);
    assert_eq!(pcm1.len(), 4410 * 2);
    let (pcm2, rate2, _) = decode_flac_bytes(&bytes[positions[1]..]);
    assert_eq!(rate2, 48_000);
    assert_eq!(pcm2.len(), 4800 * 2);
}

/// Opus (feature `encode-opus`): same specs → one Ogg stream; mismatched
/// channel counts → two chained Ogg streams (two OpusHead packets).
#[cfg(feature = "encode-opus")]
#[test]
fn gapless_opus_single_session_and_chained() {
    let dir = tempfile::tempdir().unwrap();
    let rate = 48_000u32;
    let frames = 4800usize;
    let a = dir.path().join("a.wav");
    let b = dir.path().join("b.wav");
    let c = dir.path().join("c.wav");
    write_wav(&a, rate, 2, frames, 440.0);
    write_wav(&b, rate, 2, frames, 440.0);
    write_wav(&c, rate, 1, frames, 440.0); // mono → spec mismatch

    let plan = |p: PathBuf| TranscodePlan {
        path: p,
        source_format: AudioFormat::Wav,
        target: StreamFormat::Opus,
        seek_ms: None,
    };

    // Single session: one Ogg OpusHead for both tracks.
    let (bytes, _, mode) = gapless_to_bytes(&[plan(a.clone()), plan(b)]);
    assert_eq!(mode, GaplessMode::SingleSession);
    let heads = bytes.windows(8).filter(|w| *w == b"OpusHead").count();
    assert_eq!(heads, 1, "single session must emit one OpusHead");

    // Chained: stereo then mono → two independent Ogg streams.
    let (bytes, _, mode) = gapless_to_bytes(&[plan(a), plan(c)]);
    assert_eq!(mode, GaplessMode::Chained);
    let heads = bytes.windows(8).filter(|w| *w == b"OpusHead").count();
    assert_eq!(heads, 2, "chained mode must emit two OpusHeads");
}

/// MP3 (feature `encode-mp3`): best-effort single session for matching
/// specs. Encoder delay/padding mean the two-track output is NOT
/// sample-exact at the boundary — this test only pins "runs and yields one
/// MP3 session", honest about the MP3 limitation.
#[cfg(feature = "encode-mp3")]
#[test]
fn gapless_mp3_single_session_best_effort() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    let b = dir.path().join("b.wav");
    write_wav(&a, 44_100, 2, 4410, 440.0);
    write_wav(&b, 44_100, 2, 4410, 440.0);
    let plan = |p: PathBuf| TranscodePlan {
        path: p,
        source_format: AudioFormat::Wav,
        target: StreamFormat::Mp3,
        seek_ms: None,
    };
    let (bytes, chain, mode) = gapless_to_bytes(&[plan(a), plan(b)]);
    assert_eq!(mode, GaplessMode::SingleSession);
    assert!(chain.contains("wav->mp3"), "chain: {chain}");
    assert!(!bytes.is_empty());
    // MP3 frames start with a sync word; one continuous session.
    assert!(bytes
        .windows(2)
        .any(|w| w[0] == 0xFF && w[1] & 0xE0 == 0xE0));
}
