//! Transcode pipeline: decode → (resample) → encode, streamed. (Spec §4, S6.)
//!
//! ## Architecture
//!
//! ```text
//! PcmSource (Symphonia decode | DSD FIR decimation)
//!     │  interleaved f32, bounded buffers
//!     ▼
//! optional resample (cubic; only for the lossy encode targets)
//!     │
//!     ▼
//! Encoder (FLAC | Opus | MP3)
//!     │  framed bytes
//!     ▼
//! tokio mpsc ──► axum Body (chunked, no Content-Length)
//! ```
//!
//! The producer runs on a blocking thread (`spawn_blocking` / `std::thread`)
//! and pushes framed bytes through a bounded mpsc channel; the HTTP layer
//! only does socket delivery. Nothing here ever buffers a whole track:
//! decode, resample and encode all operate on bounded blocks.
//!
//! ## Seek (S12, transcode half)
//!
//! `PcmSource::seek_pcm` seeks the decoder near the target and then
//! decode-and-drops the remainder, so the first emitted PCM sample is
//! sample-exact from the seek point onward. The *seek point itself* is only
//! as accurate as the container/codec seek: within one encoder frame for
//! MP3/AAC (~26 ms), sample-exact for FLAC/WAV/DSD. Passthrough + `seek_ms`
//! stays ignored (HTTP Range is the passthrough seek mechanism).

use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use kahawai_core::{AudioFormat, DsdStory, MusicError, StreamFormat};
use symphonia::core::audio::sample::{i24, u24, Sample};
use symphonia::core::audio::{Audio, AudioBuffer, GenericAudioBufferRef};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::dsd::{self, DsdPcmReader};

// ---------------------------------------------------------------------------
// PCM source abstraction
// ---------------------------------------------------------------------------

/// PCM stream description, in decode order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmSpec {
    pub sample_rate: u32,
    pub channels: usize,
    /// Bits per sample of the *source* (informational for the chain label).
    /// Not read by the pipeline: encoders take f32 in [-1, 1]; FLAC
    /// quantizes to 24-bit.
    #[allow(dead_code)]
    pub bits_per_sample: u8,
}

/// Anything that yields interleaved f32 PCM frames.
pub trait PcmSource: Send {
    fn spec(&self) -> PcmSpec;
    /// Fill `out` (len a multiple of channels) with interleaved f32.
    /// Returns the number of f32 samples written; 0 at end of stream.
    fn fill(&mut self, out: &mut [f32]) -> Result<usize, MusicError>;
    /// Seek so the next [`PcmSource::fill`] starts at PCM frame `frame`.
    /// Decode-and-drop: sample-exact from the seek point onward.
    fn seek_pcm(&mut self, frame: u64) -> Result<(), MusicError>;
}

// ---------------------------------------------------------------------------
// Symphonia decode source (all directly-decodable formats)
// ---------------------------------------------------------------------------

fn symphonia_error(e: SymphoniaError) -> MusicError {
    match e {
        SymphoniaError::IoError(e) => MusicError::Io(e),
        SymphoniaError::DecodeError(_) | SymphoniaError::SeekError(_) => {
            MusicError::BadRequest(format!("decode error: {e}"))
        }
        SymphoniaError::Unsupported(_) => MusicError::UnsupportedFormat(format!("symphonia: {e}")),
        other => MusicError::BadRequest(format!("symphonia: {other}")),
    }
}

/// Symphonia-backed PCM source: MP3/FLAC/AAC/M4A/WAV/AIFF/Ogg Vorbis/Opus.
pub struct SymphoniaSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    spec: PcmSpec,
    /// Buffered decoded PCM (interleaved f32) not yet consumed.
    pending: Vec<f32>,
    pending_pos: usize,
    exhausted: bool,
}

impl SymphoniaSource {
    pub fn open(path: &Path) -> Result<Self, MusicError> {
        let file = File::open(path).map_err(MusicError::Io)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        // Opus decode via the libopus adapter (spec: Opus support required).
        registry.register_audio_decoder::<symphonia_adapter_libopus::OpusDecoder>();

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
            .ok_or_else(|| MusicError::BadRequest("no audio track found".into()))?;
        let track_id = track.id;
        let params = match track.codec_params.as_ref() {
            Some(symphonia::core::codecs::CodecParameters::Audio(p)) => p,
            _ => return Err(MusicError::BadRequest("no audio codec parameters".into())),
        };

        let sample_rate = params
            .sample_rate
            .ok_or_else(|| MusicError::BadRequest("unknown sample rate".into()))?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .ok_or_else(|| MusicError::BadRequest("unknown channel count".into()))?;
        if channels == 0 || channels > 8 {
            return Err(MusicError::BadRequest(format!(
                "unsupported channel count {channels}"
            )));
        }
        let bits_per_sample = params.bits_per_sample.unwrap_or(16) as u8;

        let decoder = registry
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(symphonia_error)?;

        Ok(Self {
            format,
            decoder,
            track_id,
            spec: PcmSpec {
                sample_rate,
                channels,
                bits_per_sample,
            },
            pending: Vec::new(),
            pending_pos: 0,
            exhausted: false,
        })
    }

    /// Decode one packet into `pending`. Returns false at end of stream.
    fn decode_packet(&mut self) -> Result<bool, MusicError> {
        let packet = match self.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(false),
            Err(e) => return Err(symphonia_error(e)),
        };
        if packet.track_id != self.track_id {
            return Ok(true);
        }
        let decoded = self.decoder.decode(&packet).map_err(symphonia_error)?;
        append_audio_ref(&mut self.pending, decoded);
        Ok(true)
    }
}

impl PcmSource for SymphoniaSource {
    fn spec(&self) -> PcmSpec {
        self.spec
    }

    fn fill(&mut self, out: &mut [f32]) -> Result<usize, MusicError> {
        let ch = self.spec.channels;
        assert!(out.len().is_multiple_of(ch));
        let mut written = 0;
        while written < out.len() {
            let avail = self.pending.len() - self.pending_pos;
            if avail == 0 {
                if self.exhausted || !self.decode_packet()? {
                    self.exhausted = true;
                    break;
                }
                continue;
            }
            let take = avail.min(out.len() - written);
            out[written..written + take]
                .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + take]);
            self.pending_pos += take;
            written += take;
            if self.pending_pos == self.pending.len() {
                self.pending.clear();
                self.pending_pos = 0;
            }
        }
        Ok(written)
    }

    fn seek_pcm(&mut self, frame: u64) -> Result<(), MusicError> {
        let track = self
            .format
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)
            .ok_or_else(|| MusicError::BadRequest("track vanished".into()))?;
        let tb = track
            .time_base
            .ok_or_else(|| MusicError::BadRequest("no time base for seek".into()))?;
        let numer = tb.numer.get() as f64;
        let denom = tb.denom.get() as f64;
        let seconds = frame as f64 / self.spec.sample_rate as f64;
        let seek_time = Time::try_from_secs_f64(seconds)
            .ok_or_else(|| MusicError::BadRequest("seek out of range".into()))?;
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: seek_time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(symphonia_error)?;

        // Convert between PCM frames and track timestamp units:
        // ts = frame * denom / (sample_rate * numer).
        let to_ts =
            |f: u64| (f as f64 * denom / (self.spec.sample_rate as f64 * numer)).round() as i64;
        let to_frames =
            |ts: i64| (ts as f64 * self.spec.sample_rate as f64 * numer / denom).round() as u64;
        let wanted_ts = to_ts(frame);
        let actual_ts = seeked.actual_ts.get();
        let drop_frames = to_frames(wanted_ts.saturating_sub(actual_ts.min(wanted_ts)));

        self.decoder.reset();
        self.pending.clear();
        self.pending_pos = 0;
        self.exhausted = false;

        // Decode-and-drop the remainder: sample-exact from here on.
        let ch = self.spec.channels;
        let mut to_drop = drop_frames as usize * ch;
        let mut tmp = vec![0.0f32; 8192 * ch];
        while to_drop > 0 {
            let take = to_drop.min(tmp.len());
            let n = self.fill(&mut tmp[..take])?;
            if n == 0 {
                break; // seek ran past EOF
            }
            to_drop -= n;
        }
        Ok(())
    }
}

/// Copy any symphonia audio buffer into interleaved f32.
fn append_audio_ref(out: &mut Vec<f32>, buf: GenericAudioBufferRef) {
    match buf {
        GenericAudioBufferRef::F32(b) => push_planar(out, b, |v: f32| v),
        GenericAudioBufferRef::F64(b) => push_planar(out, b, |v: f64| v as f32),
        GenericAudioBufferRef::S8(b) => push_planar(out, b, |v: i8| v as f32 / 128.0),
        GenericAudioBufferRef::S16(b) => push_planar(out, b, |v: i16| v as f32 / 32768.0),
        GenericAudioBufferRef::S24(b) => {
            push_planar(out, b, |v: i24| v.inner() as f32 / 8_388_608.0)
        }
        GenericAudioBufferRef::S32(b) => push_planar(out, b, |v: i32| v as f32 / 2_147_483_648.0),
        GenericAudioBufferRef::U8(b) => push_planar(out, b, |v: u8| (v as f32 - 128.0) / 128.0),
        GenericAudioBufferRef::U16(b) => {
            push_planar(out, b, |v: u16| (v as f32 - 32768.0) / 32768.0)
        }
        GenericAudioBufferRef::U24(b) => push_planar(out, b, |v: u24| {
            (v.inner() as f32 - 8_388_608.0) / 8_388_608.0
        }),
        GenericAudioBufferRef::U32(b) => push_planar(out, b, |v: u32| {
            (v as f32 - 2_147_483_648.0) / 2_147_483_648.0
        }),
    }
}

/// Generic planar→interleaved copy for any sample type and channel count.
fn push_planar<T, F>(out: &mut Vec<f32>, buf: &AudioBuffer<T>, conv: F)
where
    T: Sample,
    F: Fn(T) -> f32,
{
    let channels = buf.spec().channels().count();
    let frames = buf.frames();
    out.reserve(frames * channels);
    for f in 0..frames {
        for c in 0..channels {
            let s = buf
                .plane(c)
                .and_then(|p| p.get(f))
                .copied()
                .unwrap_or(T::MID);
            out.push(conv(s));
        }
    }
}

// ---------------------------------------------------------------------------
// DSD → PCM source (S5a)
// ---------------------------------------------------------------------------

/// DSD file decoded through the in-house FIR decimator ([`crate::dsd`]).
pub struct DsdPcmSource<R: Read + Seek + Send> {
    reader: DsdPcmReader<R>,
    spec: PcmSpec,
}

impl DsdPcmSource<std::io::BufReader<File>> {
    pub fn open(path: &Path) -> Result<Self, MusicError> {
        let file = File::open(path).map_err(MusicError::Io)?;
        let reader = DsdPcmReader::open(std::io::BufReader::new(file))?;
        Ok(Self {
            spec: PcmSpec {
                sample_rate: reader.pcm_rate(),
                channels: reader.channels(),
                bits_per_sample: 24, // decimator output is quantized to 24-bit
            },
            reader,
        })
    }

    /// Chain label fragment, e.g. `dsf64`.
    pub fn chain_label(&self) -> &'static str {
        dsd::dsd_chain_label(self.reader.info().dsd_rate, self.reader.info().is_dsf)
    }

    /// FIR tap count, for logs/diagnostics.
    pub fn taps(&self) -> usize {
        self.reader.taps()
    }
}

impl<R: Read + Seek + Send> PcmSource for DsdPcmSource<R> {
    fn spec(&self) -> PcmSpec {
        self.spec
    }

    fn fill(&mut self, out: &mut [f32]) -> Result<usize, MusicError> {
        self.reader.read_pcm(out)
    }

    fn seek_pcm(&mut self, frame: u64) -> Result<(), MusicError> {
        self.reader.skip_pcm(frame)
    }
}

// ---------------------------------------------------------------------------
// Streaming FLAC encoder (flacenc, constant memory)
// ---------------------------------------------------------------------------

use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, StreamInfo};
use flacenc::config;
use flacenc::error::Verify;
use flacenc::source::{Fill, FrameBuf};

/// FLAC encode target: 24-bit (spec §4: lossless ladder rung).
const FLAC_BITS: usize = 24;
/// Encoder block size in frames: 4096 is flacenc's default and a good
/// latency/throughput trade for streaming.
const FLAC_BLOCK_SIZE: usize = 4096;

/// Incremental FLAC encoder: emits one container header, then encoded
/// frames as PCM blocks are pushed. Memory is O(block size).
pub struct FlacStreamEncoder {
    config: flacenc::error::Verified<config::Encoder>,
    stream_info: StreamInfo,
    frame_number: usize,
    block_size: usize,
    sample_rate: u32,
    channels: usize,
    /// Accumulated interleaved i32 samples (< block_size frames).
    pending: Vec<i32>,
}

impl FlacStreamEncoder {
    pub fn new(sample_rate: u32, channels: usize) -> Result<Self, MusicError> {
        let mut cfg = config::Encoder::default();
        cfg.block_size = FLAC_BLOCK_SIZE;
        // Streaming: deterministic single-threaded encode; the HTTP layer
        // already parallelizes across connections.
        cfg.multithread = false;
        let config = cfg
            .into_verified()
            .map_err(|e| MusicError::BadRequest(format!("flac config: {e:?}")))?;
        let stream_info = StreamInfo::new(sample_rate as usize, channels, FLAC_BITS)
            .map_err(|e| MusicError::BadRequest(format!("flac streaminfo: {e}")))?;
        Ok(Self {
            config,
            stream_info,
            frame_number: 0,
            block_size: FLAC_BLOCK_SIZE,
            sample_rate,
            channels,
            pending: Vec::with_capacity(FLAC_BLOCK_SIZE * channels),
        })
    }

    /// Container header: `fLaC` magic + one STREAMINFO metadata block.
    ///
    /// `total_samples` is 0 (unknown — we stream) and the MD5 is zeroed
    /// (verification disabled). This is the standard streaming-FLAC header
    /// Written manually (rather than via flacenc's metadata writer) so the
    /// min/max block-size fields carry our real fixed block size; flacenc
    /// leaves them as unknown sentinels that strict decoders reject.
    pub fn header_bytes(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(42);
        h.extend_from_slice(b"fLaC");
        // Metadata block header: last-block flag (1) + type 0 (STREAMINFO)
        // + 24-bit length 34.
        h.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
        // min/max block size: our fixed block size (the final partial block
        // is the only exception; frame headers carry the true sizes).
        h.extend_from_slice(&(FLAC_BLOCK_SIZE as u16).to_be_bytes());
        h.extend_from_slice(&(FLAC_BLOCK_SIZE as u16).to_be_bytes());
        // min/max frame size: unknown → 0.
        h.extend_from_slice(&[0u8; 6]);
        // 20-bit sample rate | 3-bit (channels-1) | 5-bit (bps-1) |
        // 36-bit total samples (0 = unknown).
        let sr = self.sample_rate;
        let ch = self.channels as u32;
        let bps1 = FLAC_BITS as u32 - 1;
        h.push((sr >> 12) as u8);
        h.push((sr >> 4) as u8);
        h.push((((sr & 0xF) << 4) | ((ch - 1) << 1) | (bps1 >> 4)) as u8);
        h.push(((bps1 & 0xF) << 4) as u8); // top 4 bits of total_samples = 0
        h.extend_from_slice(&[0u8; 4]); // remaining 32 bits of total_samples
                                        // MD5 of unencoded audio: 0 = verification disabled.
        h.extend_from_slice(&[0u8; 16]);
        debug_assert_eq!(h.len(), 42);
        h
    }

    /// Push interleaved f32 PCM in [-1, 1]; returns encoded frame bytes.
    pub fn push_f32(&mut self, pcm: &[f32]) -> Result<Vec<u8>, MusicError> {
        assert!(pcm.len().is_multiple_of(self.channels));
        let scale = (1i32 << (FLAC_BITS - 1)) as f32;
        self.pending.reserve(pcm.len());
        for &s in pcm {
            let v = (s * scale).round().clamp(-scale, scale - 1.0) as i32;
            self.pending.push(v);
        }
        self.emit_full_blocks()
    }

    /// Encode and flush any partial final block. After this the stream is
    /// complete (FLAC has no trailer).
    pub fn finish(&mut self) -> Result<Vec<u8>, MusicError> {
        let mut out = self.emit_full_blocks()?;
        if !self.pending.is_empty() {
            out.extend(self.encode_one_block(true)?);
        }
        Ok(out)
    }

    fn emit_full_blocks(&mut self) -> Result<Vec<u8>, MusicError> {
        let mut out = Vec::new();
        while self.pending.len() >= self.block_size * self.channels {
            out.extend(self.encode_one_block(false)?);
        }
        Ok(out)
    }

    fn encode_one_block(&mut self, partial_ok: bool) -> Result<Vec<u8>, MusicError> {
        let frames = if partial_ok {
            self.pending.len() / self.channels
        } else {
            self.block_size
        };
        let take = frames * self.channels;
        let mut fb = FrameBuf::with_size(self.channels, self.block_size)
            .map_err(|e| MusicError::BadRequest(format!("flac framebuf: {e}")))?;
        fb.fill_interleaved(&self.pending[..take])
            .map_err(|e| MusicError::BadRequest(format!("flac fill: {e:?}")))?;
        self.pending.drain(..take);
        let frame = flacenc::encode_fixed_size_frame(
            &self.config,
            &fb,
            self.frame_number,
            &self.stream_info,
        )
        .map_err(|e| MusicError::BadRequest(format!("flac encode: {e}")))?;
        self.frame_number += 1;
        let mut sink = ByteSink::new();
        frame
            .write(&mut sink)
            .map_err(|e| MusicError::BadRequest(format!("flac write: {e:?}")))?;
        Ok(sink.as_slice().to_vec())
    }
}

// ---------------------------------------------------------------------------
// Ogg/Opus encoder (feature: encode-opus)
// ---------------------------------------------------------------------------

/// Ogg page writer: just enough of RFC 3533 to mux an Opus stream.
/// One packet per page keeps lacing trivial and streams fine.
#[cfg(feature = "encode-opus")]
mod ogg {
    /// Build the 256-entry CRC table for the Ogg polynomial 0x04C11DB7.
    pub fn crc_table() -> [u32; 256] {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 {
                    (r << 1) ^ 0x04C1_1DB7
                } else {
                    r << 1
                };
            }
            *slot = r;
        }
        table
    }

    fn crc32(data: &[u8], table: &[u32; 256]) -> u32 {
        let mut crc = 0u32;
        for &b in data {
            crc = table[((crc >> 24) ^ b as u32) as usize] ^ (crc << 8);
        }
        crc
    }

    /// Assemble one Ogg page carrying exactly one packet.
    pub fn page(
        packet: &[u8],
        header_type: u8,
        granule: u64,
        serial: u32,
        seqno: u32,
        table: &[u32; 256],
    ) -> Vec<u8> {
        // Segment table: packet split into 255-byte chunks + a final
        // short segment (a trailing 255 would mean "continued").
        let full = packet.len() / 255;
        let rem = packet.len() % 255;
        let nseg = full + 1; // rem < 255 always terminates the packet
        debug_assert!(nseg <= 255);

        let mut page = Vec::with_capacity(27 + nseg + packet.len());
        page.extend_from_slice(b"OggS");
        page.push(0); // version
        page.push(header_type);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&serial.to_le_bytes());
        page.extend_from_slice(&seqno.to_le_bytes());
        let crc_pos = page.len();
        page.extend_from_slice(&[0, 0, 0, 0]); // CRC placeholder
        page.push(nseg as u8);
        for _ in 0..full {
            page.push(255);
        }
        page.push(rem as u8);
        page.extend_from_slice(packet);

        let crc = crc32(&page, table);
        page[crc_pos..crc_pos + 4].copy_from_slice(&crc.to_le_bytes());
        page
    }

    pub fn opus_head(channels: usize, pre_skip: u16) -> Vec<u8> {
        let mut p = Vec::with_capacity(19);
        p.extend_from_slice(b"OpusHead");
        p.push(1); // version
        p.push(channels as u8);
        p.extend_from_slice(&pre_skip.to_le_bytes());
        p.extend_from_slice(&48_000u32.to_le_bytes()); // input sample rate
        p.extend_from_slice(&0i16.to_le_bytes()); // output gain
        p.push(0); // channel mapping family 0
        p
    }

    pub fn opus_tags() -> Vec<u8> {
        let vendor = b"kahawai";
        let mut p = Vec::new();
        p.extend_from_slice(b"OpusTags");
        p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        p.extend_from_slice(vendor);
        p.extend_from_slice(&0u32.to_le_bytes()); // no user comments
        p
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn page_crc_verifies() {
            let table = crc_table();
            let page = page(b"hello", 0x02, 0, 1234, 0, &table);
            // Recompute CRC over the page with the CRC field zeroed.
            let mut check = page.clone();
            check[22..26].copy_from_slice(&[0, 0, 0, 0]);
            let crc = crc32(&check, &table);
            assert_eq!(&page[22..26], &crc.to_le_bytes());
            assert_eq!(&page[0..4], b"OggS");
        }
    }
}

/// Streaming Ogg/Opus encoder: 48 kHz in, Ogg pages out.
///
/// Raw pointer to the libopus state; never shared between threads, so
/// `Send` is sound (the producer thread owns it exclusively).
#[cfg(feature = "encode-opus")]
pub struct OpusOggEncoder {
    enc: *mut opusic_sys::OpusEncoder,
    pending: Vec<f32>, // interleaved 48 kHz f32
    channels: usize,
    serial: u32,
    seqno: u32,
    /// 48 kHz samples emitted so far, INCLUDING the pre-skip region
    /// (RFC 7845 §4 granule semantics: starts at pre_skip).
    granule: u64,
    crc_table: [u32; 256],
    header_sent: bool,
    frame_size: usize, // 960 = 20 ms @ 48 kHz
    /// Encoder lookahead in 48 kHz samples → OpusHead pre-skip.
    preskip: u16,
    /// Real (non-padding) input samples; EOS granule trims the tail pad.
    input_samples: u64,
}

// SAFETY: the libopus encoder is only touched on the owning producer thread.
#[cfg(feature = "encode-opus")]
unsafe impl Send for OpusOggEncoder {}

#[cfg(feature = "encode-opus")]
impl OpusOggEncoder {
    /// Bitrate in bits/sec (v1 default: 128k stereo, 96k mono).
    pub fn new(channels: usize, bitrate: i32) -> Result<Self, MusicError> {
        let mut err = 0;
        // SAFETY: valid args; error checked below.
        let enc = unsafe {
            opusic_sys::opus_encoder_create(
                48000,
                channels as core::ffi::c_int,
                opusic_sys::OPUS_APPLICATION_AUDIO,
                &mut err,
            )
        };
        if err != opusic_sys::OPUS_OK || enc.is_null() {
            return Err(MusicError::BadRequest(format!(
                "opus_encoder_create failed: {err}"
            )));
        }
        // SAFETY: enc is valid; single int arg by value.
        let rc = unsafe {
            opusic_sys::opus_encoder_ctl(enc, opusic_sys::OPUS_SET_BITRATE_REQUEST, bitrate)
        };
        if rc != opusic_sys::OPUS_OK {
            unsafe { opusic_sys::opus_encoder_destroy(enc) };
            return Err(MusicError::BadRequest(format!("opus set bitrate: {rc}")));
        }
        // Query the real lookahead for OpusHead pre-skip (typically 120
        // samples = 2.5 ms at 48 kHz — NOT the 960-sample frame size).
        let mut lookahead: core::ffi::c_int = 0;
        // SAFETY: GET_LOOKAHEAD writes one int through the pointer.
        let rc = unsafe {
            opusic_sys::opus_encoder_ctl(
                enc,
                opusic_sys::OPUS_GET_LOOKAHEAD_REQUEST,
                &mut lookahead as *mut core::ffi::c_int,
            )
        };
        if rc != opusic_sys::OPUS_OK || lookahead <= 0 || lookahead > u16::MAX as i32 {
            unsafe { opusic_sys::opus_encoder_destroy(enc) };
            return Err(MusicError::BadRequest(format!("opus get lookahead: {rc}")));
        }
        let preskip = lookahead as u16;
        // RFC 7845 §4: granule positions count decoded samples starting at
        // sample 0, which *includes* the pre-skip region — so the running
        // granule starts at pre_skip, not 0.
        let granule = preskip as u64;
        Ok(Self {
            enc,
            pending: Vec::with_capacity(960 * channels * 2),
            channels,
            serial: rand_serial(),
            seqno: 0,
            granule,
            crc_table: ogg::crc_table(),
            header_sent: false,
            frame_size: 960,
            preskip,
            input_samples: 0,
        })
    }

    /// Ogg header pages (BOS OpusHead + OpusTags). Sent once, first.
    pub fn header_bytes(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        // Pre-skip = encoder lookahead queried above (RFC 7845 §5.1).
        let head = ogg::page(
            &ogg::opus_head(self.channels, self.preskip),
            0x02,
            0,
            self.serial,
            self.seqno,
            &self.crc_table,
        );
        self.seqno += 1;
        let tags = ogg::page(
            &ogg::opus_tags(),
            0x00,
            0,
            self.serial,
            self.seqno,
            &self.crc_table,
        );
        self.seqno += 1;
        out.extend(head);
        out.extend(tags);
        self.header_sent = true;
        out
    }

    /// Push interleaved 48 kHz f32; returns complete Ogg pages.
    pub fn push_f32(&mut self, pcm: &[f32]) -> Result<Vec<u8>, MusicError> {
        assert!(pcm.len() % self.channels == 0);
        self.pending.extend_from_slice(pcm);
        self.emit_frames(false)
    }

    /// Flush: encode the tail (zero-padded) and close with an EOS page.
    pub fn finish(&mut self) -> Result<Vec<u8>, MusicError> {
        let mut out = self.emit_frames(true)?;
        // Empty final packet with EOS flag carries the final granule:
        // pre_skip + real input samples (RFC 7845 §4), so decoders trim
        // the zero padding we added to fill the last frame.
        let eos = ogg::page(
            &[],
            0x04,
            self.preskip as u64 + self.input_samples,
            self.serial,
            self.seqno,
            &self.crc_table,
        );
        self.seqno += 1;
        out.extend(eos);
        Ok(out)
    }

    fn emit_frames(&mut self, flush: bool) -> Result<Vec<u8>, MusicError> {
        let mut out = Vec::new();
        let want = self.frame_size * self.channels;
        while self.pending.len() >= want || (flush && !self.pending.is_empty()) {
            let mut frame = vec![0.0f32; want];
            let n = self.pending.len().min(want);
            frame[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            self.input_samples += (n / self.channels) as u64;

            let mut packet = vec![0u8; 4000];
            // SAFETY: enc valid, frame/max sizes honored, packet buffer sized.
            let len = unsafe {
                opusic_sys::opus_encode_float(
                    self.enc,
                    frame.as_ptr(),
                    self.frame_size as core::ffi::c_int,
                    packet.as_mut_ptr(),
                    packet.len() as opusic_sys::opus_int32,
                )
            };
            if len < 0 {
                return Err(MusicError::BadRequest(format!("opus_encode: {len}")));
            }
            packet.truncate(len as usize);
            self.granule += self.frame_size as u64;
            let page = ogg::page(
                &packet,
                0x00,
                self.granule,
                self.serial,
                self.seqno,
                &self.crc_table,
            );
            self.seqno += 1;
            out.extend(page);
        }
        Ok(out)
    }
}

#[cfg(feature = "encode-opus")]
impl Drop for OpusOggEncoder {
    fn drop(&mut self) {
        // SAFETY: enc was created successfully and not yet destroyed.
        unsafe { opusic_sys::opus_encoder_destroy(self.enc) };
    }
}

#[cfg(feature = "encode-opus")]
fn rand_serial() -> u32 {
    // Not cryptographic: just needs to be a plausible stream serial.
    // Mix time + address bits; collisions across streams are harmless.
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0x1234_5678);
    t ^ 0x9E37_79B9
}

// ---------------------------------------------------------------------------
// MP3 encoder (feature: encode-mp3) — LAME, CBR
// ---------------------------------------------------------------------------

/// Streaming MP3 encoder via bundled LAME: CBR, source rate (≤48 kHz).
///
/// Raw pointer to the LAME state; owned exclusively by the producer thread.
#[cfg(feature = "encode-mp3")]
pub struct Mp3Encoder {
    gfp: *mut mp3lame_sys::lame_global_flags,
    pending_l: Vec<f32>,
    pending_r: Vec<f32>,
    channels: usize,
    frame_samples: usize, // 1152 per MP3 frame
}

// SAFETY: LAME handle only touched on the owning producer thread.
#[cfg(feature = "encode-mp3")]
unsafe impl Send for Mp3Encoder {}

#[cfg(feature = "encode-mp3")]
impl Mp3Encoder {
    /// `bitrate_kbps`: v1 default 192.
    pub fn new(sample_rate: u32, channels: usize, bitrate_kbps: i32) -> Result<Self, MusicError> {
        // SAFETY: no args; null checked below.
        let gfp = unsafe { mp3lame_sys::lame_init() };
        if gfp.is_null() {
            return Err(MusicError::BadRequest("lame_init failed".into()));
        }
        // SAFETY: gfp valid from here on; each setter's return is checked.
        unsafe {
            let ok = |rc: core::ffi::c_int, what: &str| -> Result<(), MusicError> {
                if rc == 0 {
                    Ok(())
                } else {
                    mp3lame_sys::lame_close(gfp);
                    Err(MusicError::BadRequest(format!("lame {what}: {rc}")))
                }
            };
            ok(
                mp3lame_sys::lame_set_in_samplerate(gfp, sample_rate as core::ffi::c_int),
                "in_samplerate",
            )?;
            ok(
                mp3lame_sys::lame_set_num_channels(gfp, channels as core::ffi::c_int),
                "num_channels",
            )?;
            ok(
                mp3lame_sys::lame_set_brate(gfp, bitrate_kbps as core::ffi::c_int),
                "brate",
            )?;
            ok(
                mp3lame_sys::lame_set_VBR(gfp, mp3lame_sys::vbr_mode::vbr_off),
                "vbr_off",
            )?;
            ok(mp3lame_sys::lame_set_bWriteVbrTag(gfp, 0), "vbr_tag")?;
            if mp3lame_sys::lame_init_params(gfp) != 0 {
                mp3lame_sys::lame_close(gfp);
                return Err(MusicError::BadRequest("lame_init_params failed".into()));
            }
        }
        Ok(Self {
            gfp,
            pending_l: Vec::with_capacity(1152 * 2),
            pending_r: Vec::with_capacity(1152 * 2),
            channels,
            frame_samples: 1152,
        })
    }

    /// MP3 has no container header; the byte stream starts with frames.
    pub fn header_bytes(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Push interleaved f32 at the encoder's sample rate; returns MP3 frames.
    pub fn push_f32(&mut self, pcm: &[f32]) -> Result<Vec<u8>, MusicError> {
        assert!(pcm.len() % self.channels == 0);
        for f in pcm.chunks_exact(self.channels) {
            self.pending_l.push(f[0]);
            self.pending_r
                .push(if self.channels > 1 { f[1] } else { f[0] });
        }
        self.emit_frames(false)
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, MusicError> {
        let mut out = self.emit_frames(true)?;
        let mut mp3buf = vec![0u8; 7200];
        // SAFETY: gfp valid; buffer sized per LAME docs.
        let n = unsafe {
            mp3lame_sys::lame_encode_flush(
                self.gfp,
                mp3buf.as_mut_ptr(),
                mp3buf.len() as core::ffi::c_int,
            )
        };
        if n < 0 {
            return Err(MusicError::BadRequest(format!("lame_encode_flush: {n}")));
        }
        out.extend_from_slice(&mp3buf[..n as usize]);
        Ok(out)
    }

    fn emit_frames(&mut self, flush: bool) -> Result<Vec<u8>, MusicError> {
        let mut out = Vec::new();
        while self.pending_l.len() >= self.frame_samples || (flush && !self.pending_l.is_empty()) {
            let n = self.pending_l.len().min(self.frame_samples);
            let mut l = vec![0.0f32; self.frame_samples];
            let mut r = vec![0.0f32; self.frame_samples];
            l[..n].copy_from_slice(&self.pending_l[..n]);
            r[..n].copy_from_slice(&self.pending_r[..n]);
            self.pending_l.drain(..n);
            self.pending_r.drain(..n);

            let mut mp3buf = vec![0u8; self.frame_samples * 5 / 4 + 7200];
            // SAFETY: gfp valid; pcm/mp3 buffers sized; nsamples ≤ 1152.
            let got = unsafe {
                mp3lame_sys::lame_encode_buffer_ieee_float(
                    self.gfp,
                    l.as_ptr(),
                    r.as_ptr(),
                    n as core::ffi::c_int,
                    mp3buf.as_mut_ptr(),
                    mp3buf.len() as core::ffi::c_int,
                )
            };
            if got < 0 {
                return Err(MusicError::BadRequest(format!("lame_encode: {got}")));
            }
            out.extend_from_slice(&mp3buf[..got as usize]);
        }
        Ok(out)
    }
}

#[cfg(feature = "encode-mp3")]
impl Drop for Mp3Encoder {
    fn drop(&mut self) {
        // SAFETY: gfp was successfully initialized.
        unsafe {
            mp3lame_sys::lame_close(self.gfp);
        }
    }
}

// ---------------------------------------------------------------------------
// Plan, preparation, and streaming body (S6, S12, S13)
// ---------------------------------------------------------------------------
use crate::resample::CubicResampler;
use kahawai_core::transcode_ladder;
use tokio::sync::mpsc;

/// Resolved transcode job: everything the blocking producer needs.
#[derive(Debug)]
pub struct TranscodePlan {
    pub path: PathBuf,
    pub source_format: AudioFormat,
    pub target: StreamFormat, // Flac | Opus | Mp3 — never Passthrough
    pub seek_ms: Option<u64>,
}

impl TranscodePlan {
    /// One-line description for logs, e.g. `wav → flac (seek 30000ms)`.
    pub fn describe(&self) -> String {
        let seek = self
            .seek_ms
            .map(|ms| format!(" seek={ms}ms"))
            .unwrap_or_default();
        format!(
            "{} → {:?}{seek} {}",
            source_label(self.source_format),
            self.target,
            self.path.display()
        )
    }
}

/// Resolve the effective target for a stream request (S13).
///
/// Returns `Ok(None)` when the answer is passthrough. `requested` is the
/// explicit `?format=`; `ladder` is the server's preferred ladder used when
/// `requested` is absent.
pub fn resolve_plan(
    source_format: AudioFormat,
    path: PathBuf,
    requested: Option<StreamFormat>,
    ladder: &[StreamFormat],
    dsd_story: DsdStory,
    seek_ms: Option<u64>,
) -> Result<Option<TranscodePlan>, MusicError> {
    let is_dsd = matches!(
        source_format,
        AudioFormat::Dsf | AudioFormat::Dff | AudioFormat::SacdIso
    );
    let is_native_dsd = matches!(source_format, AudioFormat::Dsf | AudioFormat::Dff);

    // Explicit ?format=dop on a non-DSD source: clear 4xx. (Checked before
    // the ladder so a WAV/FLAC can never fall through to the PCM pipeline
    // with a Dop target.)
    if requested == Some(StreamFormat::Dop) && !is_native_dsd {
        if source_format == AudioFormat::SacdIso {
            return Err(iso_unsupported());
        }
        return Err(MusicError::BadRequest(format!(
            "format=dop requires a DSD (DSF/DFF) source, got {source_format:?}"
        )));
    }

    if is_dsd {
        // SACD ISO is always offline-only (spec §2), regardless of story.
        if source_format == AudioFormat::SacdIso {
            return Err(iso_unsupported());
        }
        match dsd_story {
            // S5b: the native story resolves DSF/DFF to DoP when there is
            // no explicit format, or when ?format=dop is explicit.
            DsdStory::Native if requested.is_none() || requested == Some(StreamFormat::Dop) => {
                return Ok(Some(dop_plan(path, source_format, seek_ms)));
            }
            // Explicit non-DoP format under the native story (e.g.
            // ?format=flac): honored via the normal ladder below.
            DsdStory::Native => {}
            // Explicit ?format=dop wins over the pcm story default.
            DsdStory::Pcm if requested == Some(StreamFormat::Dop) => {
                return Ok(Some(dop_plan(path, source_format, seek_ms)));
            }
            DsdStory::Pcm => {}
        }
    }

    let preference: Vec<StreamFormat> = match requested {
        Some(f) => vec![f],
        None => ladder.to_vec(),
    };
    let target = transcode_ladder(source_format, &preference);
    if target == StreamFormat::Passthrough {
        return Ok(None);
    }
    // FLAC → FLAC is a no-op: the source bytes already are the target.
    if target == StreamFormat::Flac && source_format == AudioFormat::Flac {
        return Ok(None);
    }
    // Feature gates for the C encoders (checked again at setup).
    if target == StreamFormat::Opus && cfg!(not(feature = "encode-opus")) {
        return Err(opus_disabled());
    }
    if target == StreamFormat::Mp3 && cfg!(not(feature = "encode-mp3")) {
        return Err(mp3_disabled());
    }
    Ok(Some(TranscodePlan {
        path,
        source_format,
        target,
        seek_ms,
    }))
}

fn iso_unsupported() -> MusicError {
    MusicError::UnsupportedFormat(
        "SACD ISO is decoded offline via sacd_extract (spec §2); \
         the server does not decode .iso directly"
            .into(),
    )
}

/// A [`TranscodePlan`] whose target is DoP. DoP never enters the PCM
/// pipeline: the API layer routes `target == StreamFormat::Dop` to the
/// dedicated DoP streamer (S5b) instead of [`PreparedTranscode`].
fn dop_plan(path: PathBuf, source_format: AudioFormat, seek_ms: Option<u64>) -> TranscodePlan {
    TranscodePlan {
        path,
        source_format,
        target: StreamFormat::Dop,
        seek_ms,
    }
}

fn opus_disabled() -> MusicError {
    MusicError::FeatureDisabled {
        feature: "encode-opus".into(),
        detail: "Opus encoding is not enabled in this build; \
                 rebuild the server with --features encode-opus"
            .into(),
    }
}

fn mp3_disabled() -> MusicError {
    MusicError::FeatureDisabled {
        feature: "encode-mp3".into(),
        detail: "MP3 encoding is not enabled in this build; \
                 rebuild the server with --features encode-mp3"
            .into(),
    }
}

// ---------------------------------------------------------------------------
// Encoder dispatch
// ---------------------------------------------------------------------------

enum ActiveEncoder {
    Flac(FlacStreamEncoder),
    #[cfg(feature = "encode-opus")]
    Opus(OpusOggEncoder),
    #[cfg(feature = "encode-mp3")]
    Mp3(Mp3Encoder),
}

impl ActiveEncoder {
    fn header_bytes(&mut self) -> Vec<u8> {
        match self {
            Self::Flac(e) => e.header_bytes(),
            #[cfg(feature = "encode-opus")]
            Self::Opus(e) => e.header_bytes(),
            #[cfg(feature = "encode-mp3")]
            Self::Mp3(e) => e.header_bytes(),
        }
    }

    fn push_f32(&mut self, pcm: &[f32]) -> Result<Vec<u8>, MusicError> {
        match self {
            Self::Flac(e) => e.push_f32(pcm),
            #[cfg(feature = "encode-opus")]
            Self::Opus(e) => e.push_f32(pcm),
            #[cfg(feature = "encode-mp3")]
            Self::Mp3(e) => e.push_f32(pcm),
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>, MusicError> {
        match self {
            Self::Flac(e) => e.finish(),
            #[cfg(feature = "encode-opus")]
            Self::Opus(e) => e.finish(),
            #[cfg(feature = "encode-mp3")]
            Self::Mp3(e) => e.finish(),
        }
    }

    fn content_type(&self) -> &'static str {
        match self {
            Self::Flac(_) => "audio/flac",
            #[cfg(feature = "encode-opus")]
            Self::Opus(_) => "audio/ogg",
            #[cfg(feature = "encode-mp3")]
            Self::Mp3(_) => "audio/mpeg",
        }
    }
}

/// Short source label for the `X-Transcode-Chain` header.
fn source_label(fmt: AudioFormat) -> &'static str {
    match fmt {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Flac => "flac",
        AudioFormat::M4a => "m4a",
        AudioFormat::Aac => "aac",
        AudioFormat::Wav => "wav",
        AudioFormat::Aiff => "aiff",
        AudioFormat::OggVorbis => "ogg",
        AudioFormat::Opus => "opus",
        AudioFormat::Dsf | AudioFormat::Dff | AudioFormat::SacdIso => "dsd",
        AudioFormat::Unknown => "unknown",
    }
}

/// `X-Transcode-Chain` value for passthrough responses, e.g. `wav->passthrough`.
/// Pure ASCII: header values must survive `HeaderValue::to_str()`.
pub fn passthrough_chain(source: AudioFormat) -> String {
    format!("{}->passthrough", source_label(source))
}

/// 88200 → "88.2", 48000 → "48".
fn fmt_rate(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{}", rate / 1000)
    } else {
        format!("{:.1}", rate as f32 / 1000.0)
    }
}

/// Encoder leg of the `X-Transcode-Chain` value, e.g. `flac 24/88.2`,
/// `opus 128k 48`, `mp3 192k 48`. Pure function of target + source spec —
/// shared by the streaming setup and the HEAD metadata path.
fn describe_target(target: StreamFormat, spec: &PcmSpec) -> String {
    match target {
        StreamFormat::Flac => format!("flac {FLAC_BITS}/{}", fmt_rate(spec.sample_rate)),
        StreamFormat::Opus => {
            let bitrate = if spec.channels > 1 { 128_000 } else { 96_000 };
            format!("opus {}k 48", bitrate / 1000)
        }
        StreamFormat::Mp3 => {
            // LAME tops out at 48 kHz.
            let out_rate = spec.sample_rate.min(48_000);
            format!("mp3 192k {}", fmt_rate(out_rate))
        }
        // Unreachable via resolve_plan (DoP is a separate direct path);
        // defensive string only.
        StreamFormat::Dop => "dop".to_string(),
        // Unreachable via resolve_plan (passthrough returns Ok(None));
        // defensive string only.
        StreamFormat::Passthrough => "passthrough".to_string(),
    }
}

/// Chain value + content type for a transcode plan without running the
/// pipeline: opens source headers only (no seek, no encoder construction).
/// Used by HEAD so its metadata agrees with what GET would serve (S13).
/// `resolve_plan` has already rejected disabled-encoder targets, so the
/// target is encodable in this build.
///
/// Returns the content length when it is known up front: DoP streams have
/// real WAV sizes (S5b); live PCM transcodes return `None` (unknown until
/// the encode finishes).
pub fn head_transcode_meta(
    plan: &TranscodePlan,
) -> Result<(String, &'static str, Option<u64>), MusicError> {
    if plan.target == StreamFormat::Dop {
        let file = std::fs::File::open(&plan.path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                MusicError::NotFound(format!("file missing: {}", plan.path.display()))
            } else {
                MusicError::Io(e)
            }
        })?;
        let (dop_plan, _) = crate::dop::DopPlan::resolve(file, plan.source_format)?;
        let frame = plan.seek_ms.map(|ms| dop_plan.seek_frame(ms)).unwrap_or(0);
        let len = dop_plan.seeked_len(frame);
        return Ok((
            dop_plan.chain_label(),
            StreamFormat::Dop.mime_type(),
            Some(len),
        ));
    }
    let (spec, src_label): (PcmSpec, String) = match plan.source_format {
        AudioFormat::Dsf | AudioFormat::Dff => {
            let s = DsdPcmSource::open(&plan.path)?;
            (s.spec(), s.chain_label().to_string())
        }
        _ => {
            let s = SymphoniaSource::open(&plan.path)?;
            (s.spec(), source_label(plan.source_format).to_string())
        }
    };
    let chain = format!("{src_label}->{}", describe_target(plan.target, &spec));
    Ok((chain, plan.target.mime_type(), None))
}

// ---------------------------------------------------------------------------
// Prepared transcode: blocking setup, then a streaming run
// ---------------------------------------------------------------------------

/// A transcode ready to serve: header metadata plus the stateful pipeline.
/// Built on a blocking thread via [`PreparedTranscode::setup`].
pub struct PreparedTranscode {
    /// Value for the `X-Transcode-Chain` response header.
    pub chain: String,
    pub content_type: &'static str,
    source: Box<dyn PcmSource>,
    encoder: ActiveEncoder,
    resampler: Option<CubicResampler>,
    channels: usize,
    /// Source PCM spec; gapless setup compares these across tracks.
    spec: PcmSpec,
}

/// Open the decode-side PCM source for a plan (DSD decimators for DSF/DFF,
/// symphonia for everything else). Shared by [`PreparedTranscode::setup`]
/// and the gapless path.
fn open_pcm_source(plan: &TranscodePlan) -> Result<(Box<dyn PcmSource>, String), MusicError> {
    match plan.source_format {
        AudioFormat::Dsf | AudioFormat::Dff => {
            let s = DsdPcmSource::open(&plan.path)?;
            let label = s.chain_label().to_string();
            tracing::debug!(taps = s.taps(), "DSD decimator FIR tap count");
            Ok((Box::new(s), label))
        }
        _ => {
            let s = SymphoniaSource::open(&plan.path)?;
            Ok((Box::new(s), source_label(plan.source_format).to_string()))
        }
    }
}

impl PreparedTranscode {
    /// Open the source, apply `seek_ms`, and initialize the encoder.
    /// Blocking: call from `spawn_blocking`.
    pub fn setup(plan: &TranscodePlan) -> Result<Self, MusicError> {
        // 1. Open the PCM source.
        let (mut source, src_label) = open_pcm_source(plan)?;
        let spec = source.spec();

        // 2. Transcode-side seek (S12): sample-exact from the seek point on.
        if let Some(ms) = plan.seek_ms {
            let frame = ms.saturating_mul(spec.sample_rate as u64) / 1000;
            tracing::debug!(seek_ms = ms, seek_frame = frame, "transcode seek");
            source.seek_pcm(frame)?;
        }

        // 3. Encoder selection + resample decision.
        let out_desc = describe_target(plan.target, &spec);
        let (encoder, resampler) = match plan.target {
            StreamFormat::Flac => (
                ActiveEncoder::Flac(FlacStreamEncoder::new(spec.sample_rate, spec.channels)?),
                None,
            ),
            StreamFormat::Opus => {
                #[cfg(feature = "encode-opus")]
                {
                    let bitrate = if spec.channels > 1 { 128_000 } else { 96_000 };
                    let enc = OpusOggEncoder::new(spec.channels, bitrate)?;
                    let rs = (spec.sample_rate != 48_000)
                        .then(|| CubicResampler::new(spec.channels, spec.sample_rate, 48_000));
                    (ActiveEncoder::Opus(enc), rs)
                }
                #[cfg(not(feature = "encode-opus"))]
                {
                    return Err(opus_disabled());
                }
            }
            StreamFormat::Mp3 => {
                #[cfg(feature = "encode-mp3")]
                {
                    // LAME tops out at 48 kHz.
                    let out_rate = spec.sample_rate.min(48_000);
                    let enc = Mp3Encoder::new(out_rate, spec.channels, 192)?;
                    let rs = (spec.sample_rate != out_rate)
                        .then(|| CubicResampler::new(spec.channels, spec.sample_rate, out_rate));
                    (ActiveEncoder::Mp3(enc), rs)
                }
                #[cfg(not(feature = "encode-mp3"))]
                {
                    return Err(mp3_disabled());
                }
            }
            StreamFormat::Passthrough => {
                return Err(MusicError::BadRequest(
                    "passthrough must not reach the transcode pipeline".into(),
                ));
            }
            // Unreachable via resolve_plan (DoP is a separate direct path);
            // defensive error only.
            StreamFormat::Dop => {
                return Err(MusicError::BadRequest(
                    "dop must not reach the PCM transcode pipeline".into(),
                ));
            }
        };

        let chain = format!("{src_label}->{out_desc}");
        tracing::info!(chain = %chain, "transcode prepared");
        let content_type = encoder.content_type();
        Ok(Self {
            chain,
            content_type,
            source,
            encoder,
            resampler,
            channels: spec.channels,
            spec,
        })
    }

    /// Run the pipeline: pull PCM, (resample,) encode, send framed bytes.
    /// Blocking: runs on the producer thread until the stream ends, the
    /// client disconnects, or an error occurs (the error is sent once, then
    /// the body ends — the client sees a truncated stream).
    pub fn run(mut self, tx: &mpsc::Sender<Result<Bytes, MusicError>>) {
        let mut alive = emit(tx, self.encoder.header_bytes());
        let mut pcm = vec![0.0f32; 8192 * self.channels];
        let mut rs_out = vec![0.0f32; 8192 * self.channels];
        if alive {
            alive = pump_source(
                &mut *self.source,
                &mut self.resampler,
                &mut self.encoder,
                &mut pcm,
                &mut rs_out,
                tx,
            );
        }
        if !alive {
            return; // client went away; drop everything
        }
        finish_pipeline(&mut self.resampler, &mut self.encoder, &mut rs_out, tx);
    }
}

/// Pull PCM from one source through the (shared) resampler into the
/// (shared) encoder. Returns false when the client disconnected or a
/// decode/encode error was reported.
fn pump_source(
    source: &mut dyn PcmSource,
    resampler: &mut Option<CubicResampler>,
    encoder: &mut ActiveEncoder,
    pcm: &mut [f32],
    rs_out: &mut [f32],
    tx: &mpsc::Sender<Result<Bytes, MusicError>>,
) -> bool {
    let mut alive = true;
    while alive {
        let n = match source.fill(pcm) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = tx.blocking_send(Err(e));
                return false;
            }
        };
        if let Some(rs) = resampler.as_mut() {
            rs.push(&pcm[..n]);
            loop {
                let m = rs.pull(rs_out);
                if m == 0 {
                    break;
                }
                match encoder.push_f32(&rs_out[..m]) {
                    Ok(b) => alive = emit(tx, b),
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return false;
                    }
                }
                if !alive {
                    break;
                }
            }
        } else {
            match encoder.push_f32(&pcm[..n]) {
                Ok(b) => alive = emit(tx, b),
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    return false;
                }
            }
        }
    }
    alive
}

/// Flush the resampler tail, then finish the encoder. Shared by the plain
/// and gapless runs.
fn finish_pipeline(
    resampler: &mut Option<CubicResampler>,
    encoder: &mut ActiveEncoder,
    rs_out: &mut [f32],
    tx: &mpsc::Sender<Result<Bytes, MusicError>>,
) {
    if let Some(rs) = resampler.as_mut() {
        rs.flush();
        loop {
            let m = rs.pull(rs_out);
            if m == 0 {
                break;
            }
            match encoder.push_f32(&rs_out[..m]) {
                Ok(b) => {
                    if !emit(tx, b) {
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    return;
                }
            }
        }
    }
    match encoder.finish() {
        Ok(b) => {
            emit(tx, b);
        }
        Err(e) => {
            let _ = tx.blocking_send(Err(e));
        }
    }
}

// ---------------------------------------------------------------------------
// Gapless chaining (?next=) — S8
// ---------------------------------------------------------------------------

/// Gapless mode for a `?next=` transcode response, reported in the
/// `X-Gapless-Mode` header. Every honored next track also gets
/// `X-Gapless-Next: {id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaplessMode {
    /// Every track shares sample rate + channel count: one encoder session
    /// spans all tracks, so there is no encoder re-init at boundaries.
    /// True gapless.
    SingleSession,
    /// Tracks differ in PCM spec: each track gets its own encoder session
    /// and the encoded streams are concatenated. FLAC chains gaplessly;
    /// Opus chains gaplessly when the player honors chained Ogg (each
    /// stream carries correct granules and pre-skip, per the phase-3 fix);
    /// MP3 is best-effort — encoder delay/padding is inherent to the
    /// format, documented here rather than hidden.
    Chained,
}

impl GaplessMode {
    pub fn header_value(self) -> &'static str {
        match self {
            GaplessMode::SingleSession => "single-session",
            GaplessMode::Chained => "chained",
        }
    }
}

/// A transcode response spanning the current track plus its `?next=` chain.
pub struct PreparedGapless {
    /// Combined `X-Transcode-Chain` value, e.g.
    /// `wav->flac 16/44.1 + wav->flac 16/44.1`.
    pub chain: String,
    pub content_type: &'static str,
    pub mode: GaplessMode,
    kind: GaplessKind,
}

enum GaplessKind {
    // Boxed: the single-session state is ~328 bytes vs 24 for Chained;
    // without the box every GaplessKind pays the large size (clippy
    // large_enum_variant).
    SingleSession(Box<SingleSessionState>),
    Chained { segments: Vec<PreparedTranscode> },
}

/// State for a single encoder session across the whole gapless chain.
struct SingleSessionState {
    encoder: ActiveEncoder,
    resampler: Option<CubicResampler>,
    channels: usize,
    sources: Vec<Box<dyn PcmSource>>,
}

impl PreparedGapless {
    /// Build the chained response. `plans[0]` is the current track;
    /// `plans[1..]` are the `?next=` chain, each already resolved onto the
    /// current track's target by the API layer. Blocking: call from
    /// `spawn_blocking`.
    pub fn setup(plans: &[TranscodePlan]) -> Result<Self, MusicError> {
        assert!(!plans.is_empty(), "gapless setup needs at least one plan");
        let target = plans[0].target;
        let mut prepared = Vec::with_capacity(plans.len());
        for plan in plans {
            if plan.target != target {
                return Err(MusicError::BadRequest(
                    "gapless chain targets must match".into(),
                ));
            }
            prepared.push(PreparedTranscode::setup(plan)?);
        }
        let content_type = prepared[0].content_type;
        let chain = prepared
            .iter()
            .map(|p| p.chain.clone())
            .collect::<Vec<_>>()
            .join(" + ");
        // Single-session needs identical PCM specs: one encoder (and one
        // resampler, when the target needs one) then spans every track.
        let single = prepared.iter().all(|p| p.spec == prepared[0].spec);
        let (mode, kind) = if single {
            let first = prepared.remove(0);
            let mut sources = Vec::with_capacity(prepared.len() + 1);
            sources.push(first.source);
            sources.extend(prepared.into_iter().map(|p| p.source));
            tracing::info!(chain = %chain, "gapless single-session prepared");
            (
                GaplessMode::SingleSession,
                GaplessKind::SingleSession(Box::new(SingleSessionState {
                    encoder: first.encoder,
                    resampler: first.resampler,
                    channels: first.channels,
                    sources,
                })),
            )
        } else {
            tracing::info!(chain = %chain, "gapless chained prepared");
            (
                GaplessMode::Chained,
                GaplessKind::Chained { segments: prepared },
            )
        };
        Ok(Self {
            chain,
            content_type,
            mode,
            kind,
        })
    }

    /// Run the chained pipeline on the producer thread. Same contract as
    /// [`PreparedTranscode::run`].
    pub fn run(self, tx: &mpsc::Sender<Result<Bytes, MusicError>>) {
        match self.kind {
            GaplessKind::SingleSession(state) => {
                let SingleSessionState {
                    mut encoder,
                    mut resampler,
                    channels,
                    mut sources,
                } = *state;
                let mut alive = emit(tx, encoder.header_bytes());
                let mut pcm = vec![0.0f32; 8192 * channels];
                let mut rs_out = vec![0.0f32; 8192 * channels];
                for source in &mut sources {
                    if !alive {
                        break;
                    }
                    alive = pump_source(
                        &mut **source,
                        &mut resampler,
                        &mut encoder,
                        &mut pcm,
                        &mut rs_out,
                        tx,
                    );
                }
                if !alive {
                    return;
                }
                finish_pipeline(&mut resampler, &mut encoder, &mut rs_out, tx);
            }
            GaplessKind::Chained { segments } => {
                // Each segment is a full encoder session; a disconnect
                // makes the next header emit fail fast, so this cannot
                // busy-loop on a dead client.
                for seg in segments {
                    seg.run(tx);
                }
            }
        }
    }
}

/// Send one chunk; false when the client is gone.
fn emit(tx: &mpsc::Sender<Result<Bytes, MusicError>>, bytes: Vec<u8>) -> bool {
    if bytes.is_empty() {
        return true;
    }
    tx.blocking_send(Ok(Bytes::from(bytes))).is_ok()
}

/// Spawn the blocking producer and return the streaming body.
pub fn transcode_body(prepared: PreparedTranscode) -> axum::body::Body {
    let (tx, rx) = mpsc::channel::<Result<Bytes, MusicError>>(32);
    tokio::task::spawn_blocking(move || prepared.run(&tx));
    axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// Spawn the blocking producer for a `?next=` gapless chain and return the
/// streaming body. Same contract as [`transcode_body`].
pub fn gapless_body(prepared: PreparedGapless) -> axum::body::Body {
    let (tx, rx) = mpsc::channel::<Result<Bytes, MusicError>>(32);
    tokio::task::spawn_blocking(move || prepared.run(&tx));
    axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
}

#[cfg(test)]
#[path = "transcode_tests.rs"]
mod transcode_tests;
