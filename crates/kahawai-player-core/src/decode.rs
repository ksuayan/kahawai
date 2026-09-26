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
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSourceStream, ReadBytes, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;

/// Cap for a chained response body: track + `?next=`, 24-bit/192 kHz
/// stereo WAV would be ~2.3 MiB/s, so 512 MiB is generous headroom.
const CHAINED_BODY_CAP: u64 = 512 * 1024 * 1024;

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
            let mss = MediaSourceStream::new(
                Box::new(ReadOnlySource::new(SyncRead(source))),
                Default::default(),
            );
            this.probe_stream(mss)?;
        }
        Ok(this)
    }

    pub fn spec(&self) -> DecodedSpec {
        self.first_spec
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
