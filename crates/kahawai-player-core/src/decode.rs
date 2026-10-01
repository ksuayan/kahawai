//! Streaming PCM decoder over an HTTP response body.
//!
//! Decodes whatever the server sends (WAV/FLAC/MP3/AAC/Opus/OGG) into
//! interleaved `f32` PCM with the native sample rate and channel count.
//! The [`StreamDecoder`] wraps the transport's reader directly — no temp
//! files, no full-response buffering (except chained mode, below).
//!
//! Chained responses (`X-Gapless-Mode: chained`, §3.11) concatenate two
//! complete containers. In that mode the whole body is buffered first
//! (bounded: the server chains at most track + `?next=`), and each stream
//! is re-probed at the exact byte offset where the previous container
//! ended — read from the finished reader via
//! `FormatReader::into_inner().pos()`, which is the logical consumed-byte
//! count, immune to Symphonia's internal read-ahead. If a re-probe fails
//! the response is treated as one stream and the engine falls back to
//! sequential `?next=` requests, so chaining is best-effort by construction.

use std::io::{Cursor, Read, Seek, SeekFrom};

use kahawai_core::MusicError;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, ReadBytes, ReadOnlySource};
use symphonia::core::units::Time;

use crate::rangesource::SeekableRead;
use symphonia::core::meta::MetadataOptions;

/// Cap for a chained response body: track + `?next=`, 24-bit/192 kHz
/// stereo WAV would be ~2.3 MiB/s, so 512 MiB is generous headroom.
const CHAINED_BODY_CAP: u64 = 512 * 1024 * 1024;

/// Cap for buffering an MP4/M4A response (see [`StreamDecoder::new`]).
const MP4_BODY_CAP: u64 = 1024 * 1024 * 1024;

fn symphonia_error(e: SymphoniaError) -> MusicError {
    MusicError::Metadata(format!("decode error: {e}"))
}

/// Single-threaded read adapter. Symphonia's `MediaSource` requires
/// `Sync`, but a `StreamDecoder` is only ever driven from the playback
/// thread — this wrapper is never shared between threads.
struct SyncRead(Box<dyn Read + Send>);

// SAFETY: `SyncRead` is only used inside one `StreamDecoder`, which lives
// on the playback thread. `Sync` is required solely by the trait bound.
unsafe impl Sync for SyncRead {}

impl Read for SyncRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for SyncRead {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "network stream is not seekable",
        ))
    }
}

/// A seekable network source for symphonia. Like [`SyncRead`] it is only ever
/// driven from the playback thread, so `Sync` is asserted rather than real.
struct SyncSeek {
    inner: Box<dyn SeekableRead>,
    len: u64,
}

// SAFETY: see `SyncRead`; one `StreamDecoder`, one thread.
unsafe impl Sync for SyncSeek {}

impl Read for SyncSeek {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for SyncSeek {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl MediaSource for SyncSeek {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// PCM parameters of the first stream in the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedSpec {
    pub sample_rate: u32,
    pub channels: u16,
}

/// One open container; replaced when a chained stream is re-probed.
struct ActiveDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
}

pub struct StreamDecoder {
    /// Full response bytes in chained mode (`None` when streaming).
    /// Boxed so each re-probe gets a cheap clone for its `Cursor`.
    full: Option<Box<[u8]>>,
    /// Absolute buffer offset the current chained stream started at.
    /// `MediaSourceStream::pos()` is relative to the probe start, so the
    /// absolute end-of-stream offset is `stream_base + pos()`.
    stream_base: u64,
    active: Option<ActiveDecoder>,
    first_spec: DecodedSpec,
    /// Leftover interleaved samples from the last packet.
    stash: Vec<f32>,
    expect_chained: bool,
    /// Count of successfully probed streams (1 = only the first).
    pub streams_completed: usize,
    exhausted: bool,
}

impl StreamDecoder {
    pub fn new(source: Box<dyn Read + Send>, expect_chained: bool) -> Result<Self, MusicError> {
        let mut this = Self {
            full: None,
            stream_base: 0,
            active: None,
            first_spec: DecodedSpec {
                sample_rate: 0,
                channels: 0,
            },
            stash: Vec::new(),
            expect_chained,
            streams_completed: 0,
            exhausted: false,
        };
        if expect_chained {
            let mut buf = Vec::new();
            source
                .take(CHAINED_BODY_CAP)
                .read_to_end(&mut buf)
                .map_err(MusicError::Io)?;
            this.full = Some(buf.into_boxed_slice());
            this.probe_at(0)?;
        } else {
            // Sniff the container: MP4/M4A (AAC, ALAC) commonly keeps its
            // `moov` index at the END of the file, and symphonia must seek to
            // it — impossible on a network stream. Buffer those responses so
            // the demuxer gets a seekable source; everything else (FLAC, MP3,
            // WAV, Ogg…) keeps streaming with constant memory.
            let mut source = source;
            let mut head = Vec::with_capacity(12);
            (&mut source)
                .take(12)
                .read_to_end(&mut head)
                .map_err(MusicError::Io)?;
            let is_mp4 = head.len() >= 8 && &head[4..8] == b"ftyp";
            let mss = if is_mp4 {
                let mut buf = head;
                source
                    .take(MP4_BODY_CAP)
                    .read_to_end(&mut buf)
                    .map_err(MusicError::Io)?;
                MediaSourceStream::new(
                    Box::new(Cursor::new(buf.into_boxed_slice())),
                    Default::default(),
                )
            } else {
                let replay = Cursor::new(head).chain(source);
                MediaSourceStream::new(
                    Box::new(ReadOnlySource::new(SyncRead(Box::new(replay)))),
                    Default::default(),
                )
            };
            this.probe_stream(mss)?;
        }
        Ok(this)
    }

    pub fn spec(&self) -> DecodedSpec {
        self.first_spec
    }

    /// Decode a seekable source, starting at `start_ms` using the container's
    /// own index (FLAC seek table, MP4 `stco`, WAV arithmetic, Ogg bisection)
    /// instead of reading everything before it.
    ///
    /// Returns the decoder and how many frames at the start of its output sit
    /// before the exact target and must be dropped (the demuxer lands on a
    /// packet boundary at or before it). Errors, such as a container that
    /// cannot seek, mean the caller should use the forward-only path.
    pub fn new_seekable(
        source: Box<dyn SeekableRead>,
        byte_len: u64,
        start_ms: u64,
    ) -> Result<(Self, u64), MusicError> {
        let mut this = Self {
            full: None,
            stream_base: 0,
            active: None,
            first_spec: DecodedSpec {
                sample_rate: 0,
                channels: 0,
            },
            stash: Vec::new(),
            expect_chained: false,
            streams_completed: 0,
            exhausted: false,
        };
        let mss = MediaSourceStream::new(
            Box::new(SyncSeek {
                inner: source,
                len: byte_len,
            }),
            Default::default(),
        );
        this.probe_stream(mss)?;
        let skip = if start_ms > 0 {
            this.seek_to(start_ms)?
        } else {
            0
        };
        // Decode the first packet now: it is what triggers the download at the
        // new position, so the data is already on its way when playback starts.
        if !this.decode_one_packet()? {
            this.end_stream()?;
        }
        Ok((this, skip))
    }

    /// Seek the active container to `ms`; returns the frames to discard.
    fn seek_to(&mut self, ms: u64) -> Result<u64, MusicError> {
        let rate = f64::from(self.first_spec.sample_rate.max(1));
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| MusicError::BadRequest("nothing to seek".into()))?;
        let track_id = active.track_id;
        let time_base = active
            .format
            .tracks()
            .iter()
            .find(|t| t.id == track_id)
            .and_then(|t| t.time_base);
        let seeked = active
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from_millis_u64(ms),
                    track_id: Some(track_id),
                },
            )
            .map_err(symphonia_error)?;
        active.decoder.reset();
        let ticks = (seeked.required_ts.get() - seeked.actual_ts.get()).max(0) as f64;
        // Audio timebases are normally 1/sample_rate (ticks are frames);
        // otherwise convert through the base.
        let secs = match time_base {
            Some(tb) => ticks * f64::from(tb.numer.get()) / f64::from(tb.denom.get()),
            None => ticks / rate,
        };
        Ok((secs * rate).round() as u64)
    }

    /// Probe a chained stream starting at `offset` bytes into the buffer.
    fn probe_at(&mut self, offset: usize) -> Result<(), MusicError> {
        let full = self.full.clone().expect("chained mode");
        let mut cursor = Cursor::new(full);
        cursor
            .seek(SeekFrom::Start(offset as u64))
            .map_err(MusicError::Io)?;
        let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
        self.stream_base = offset as u64;
        self.probe_stream(mss)
    }

    /// Probe whatever `mss` currently points at as a fresh audio stream.
    fn probe_stream(&mut self, mss: MediaSourceStream<'static>) -> Result<(), MusicError> {
        let hint = Hint::new(); // by magic bytes; containers are self-describing
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(symphonia_error)?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| MusicError::BadRequest("response has no audio track".into()))?;
        let track_id = track.id;
        let params = match track.codec_params.as_ref() {
            Some(symphonia::core::codecs::CodecParameters::Audio(p)) => p.clone(),
            _ => return Err(MusicError::BadRequest("no audio codec parameters".into())),
        };
        let sample_rate = params
            .sample_rate
            .ok_or_else(|| MusicError::BadRequest("unknown sample rate".into()))?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count() as u16)
            .ok_or_else(|| MusicError::BadRequest("unknown channel count".into()))?;

        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();
        let decoder = registry
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(symphonia_error)?;

        if self.streams_completed == 0 {
            self.first_spec = DecodedSpec {
                sample_rate,
                channels,
            };
        }
        self.streams_completed += 1;
        self.active = Some(ActiveDecoder {
            format,
            decoder,
            track_id,
        });
        Ok(())
    }

    /// Decode one packet into the stash. Returns `Ok(true)` when more
    /// packets may follow, `Ok(false)` at clean container EOF.
    fn decode_one_packet(&mut self) -> Result<bool, MusicError> {
        let active = self.active.as_mut().expect("decoder active");
        let packet = match active.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(false),
            Err(e) => return Err(symphonia_error(e)),
        };
        if packet.track_id != active.track_id {
            return Ok(true);
        }
        let decoded = active.decoder.decode(&packet).map_err(symphonia_error)?;
        append_interleaved(&mut self.stash, decoded);
        Ok(true)
    }

    /// The current container hit EOF. In chained mode, re-probe at the
    /// exact byte offset where this container ended; otherwise finish.
    fn end_stream(&mut self) -> Result<(), MusicError> {
        let active = self.active.take();
        if !self.expect_chained || self.exhausted {
            self.exhausted = true;
            return Ok(());
        }
        let Some(a) = active else {
            self.exhausted = true;
            return Ok(());
        };
        // Logical consumed-byte count (excludes Symphonia's read-ahead),
        // made absolute: each chained probe starts its reader at
        // `stream_base`, so `pos()` is relative to that.
        let consumed = self.stream_base + a.format.into_inner().pos();
        let total = self.full.as_ref().map(|b| b.len() as u64).unwrap_or(0);
        // No forward progress (or nothing left): stop instead of looping.
        if consumed <= self.stream_base || consumed >= total {
            self.exhausted = true;
            return Ok(());
        }
        if self.probe_at(consumed as usize).is_err() {
            // Trailing bytes that don't form a stream: best-effort ends.
            self.exhausted = true;
        }
        Ok(())
    }

    /// Decode up to `out.len()` interleaved f32 samples. Returns the count
    /// of *frames* written; 0 means the response is fully consumed.
    pub fn decode_interleaved(&mut self, out: &mut [f32]) -> Result<usize, MusicError> {
        let mut written = 0;
        while written < out.len() {
            if !self.stash.is_empty() {
                let n = self.stash.len().min(out.len() - written);
                out[written..written + n].copy_from_slice(&self.stash[..n]);
                self.stash.drain(..n);
                written += n;
                continue;
            }
            if self.active.is_none() {
                break; // fully consumed
            }
            match self.decode_one_packet()? {
                true => {} // more packets available
                false => self.end_stream()?,
            }
        }
        Ok(written / self.first_spec.channels.max(1) as usize)
    }
}

/// Copy a decoded (planar) buffer into the stash as interleaved f32.
fn append_interleaved(out: &mut Vec<f32>, buf: GenericAudioBufferRef) {
    let frames = buf.frames();
    let channels = buf.spec().channels().count();
    let start = out.len();
    out.resize(start + frames * channels, 0.0);
    buf.copy_to_slice_interleaved::<f32, _>(&mut out[start..]);
}

#[cfg(test)]
mod seekable_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    const RATE: u32 = 44_100;
    const FREQ: f64 = 440.0;
    const AMP: f64 = 0.5;

    /// The sine both fixtures contain, at time `t` seconds.
    fn expected(t: f64) -> f32 {
        (AMP * (2.0 * std::f64::consts::PI * FREQ * t).sin()) as f32
    }

    fn samples(secs: u32) -> Vec<i16> {
        (0..RATE * secs)
            .map(|i| (expected(f64::from(i) / f64::from(RATE)) * 32767.0).round() as i16)
            .collect()
    }

    fn wav(secs: u32) -> Vec<u8> {
        let data: Vec<u8> = samples(secs)
            .iter()
            .flat_map(|&s| [s.to_le_bytes(), s.to_le_bytes()].concat()) // stereo
            .collect();
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&RATE.to_le_bytes());
        v.extend_from_slice(&(RATE * 4).to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&data);
        v
    }

    /// 3 s of the same stereo 440 Hz sine, FLAC-encoded by ffmpeg (a real file
    /// from a real encoder, so the seek runs against genuine FLAC framing).
    fn flac() -> Vec<u8> {
        include_bytes!("../tests/fixtures/sine440-stereo-3s.flac").to_vec()
    }

    /// A seekable source that counts the bytes actually read from it.
    struct Counting {
        inner: std::io::Cursor<Vec<u8>>,
        read: Arc<AtomicU64>,
    }

    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read.fetch_add(n as u64, Ordering::SeqCst);
            Ok(n)
        }
    }

    impl Seek for Counting {
        fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(p)
        }
    }

    fn open(bytes: Vec<u8>, start_ms: u64) -> (StreamDecoder, u64, Arc<AtomicU64>, usize) {
        let (len, read) = (bytes.len(), Arc::new(AtomicU64::new(0)));
        let src = Counting {
            inner: std::io::Cursor::new(bytes),
            read: read.clone(),
        };
        let (d, skip) =
            StreamDecoder::new_seekable(Box::new(src), len as u64, start_ms).expect("seekable");
        (d, skip, read, len)
    }

    /// Decode 2000 frames after the seek and check they are the right audio.
    fn assert_plays_from(d: &mut StreamDecoder, skip: u64, start_ms: u64) {
        let ch = d.spec().channels as usize;
        let mut buf = vec![0.0f32; (skip as usize + 2000) * ch];
        let mut got = 0;
        while got < buf.len() {
            let n = d.decode_interleaved(&mut buf[got..]).unwrap();
            if n == 0 {
                break;
            }
            got += n * ch;
        }
        assert!(got >= buf.len(), "enough audio after the seek");
        let t0 = start_ms as f64 / 1000.0;
        for i in 0..2000usize {
            let frame = skip as usize + i;
            let want = expected(t0 + i as f64 / f64::from(RATE));
            let have = buf[frame * ch];
            assert!(
                (have - want).abs() < 2e-3,
                "frame {i} after the target: {have} vs {want} (skipped {skip})"
            );
        }
    }

    #[test]
    fn a_wav_seek_lands_on_the_target_and_reads_almost_nothing_before_it() {
        let (mut d, skip, read, len) = open(wav(10), 7000);
        assert_eq!(
            d.spec(),
            DecodedSpec {
                sample_rate: RATE,
                channels: 2
            }
        );
        assert_plays_from(&mut d, skip, 7000);
        let used = read.load(Ordering::SeqCst) as usize;
        assert!(
            used < len / 4,
            "read {used} of {len} bytes: not the 70% before the target"
        );
    }

    #[test]
    fn a_flac_seek_lands_on_the_target() {
        let (mut d, skip, _read, _len) = open(flac(), 2000);
        assert_eq!(
            d.spec(),
            DecodedSpec {
                sample_rate: RATE,
                channels: 2
            }
        );
        assert!(
            skip < 4608,
            "at most one frame to discard, not seconds: {skip}"
        );
        assert_plays_from(&mut d, skip, 2000);
    }

    #[test]
    fn seeking_to_zero_plays_from_the_start() {
        let (mut d, skip, _r, _l) = open(wav(3), 0);
        assert_eq!(skip, 0);
        assert_plays_from(&mut d, 0, 0);
    }

    #[test]
    fn a_source_that_is_not_audio_is_an_error_not_a_panic() {
        let junk = vec![7u8; 5000];
        let r = StreamDecoder::new_seekable(Box::new(std::io::Cursor::new(junk)), 5000, 1000);
        assert!(r.is_err(), "the caller falls back to the forward-only path");
    }
}
