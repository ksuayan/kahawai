//! S5b — native DSD streaming via DoP (DSD over PCM), spec §2.
//!
//! A DoP frame is one 24-bit PCM sample per channel:
//!
//! ```text
//!   bit 23..16  8-bit DoP marker, alternating 0x05 / 0xFA frame by frame
//!   bit 15..0   16 DSD bits (MSB oldest)
//! ```
//!
//! The marker is carried in the most significant byte; the DSD payload in the
//! lower 16 bits. Standard DoP layout per channel, repeated for every
//! channel in the same frame — i.e. one multichannel DoP frame is
//! `channels` consecutive 24-bit samples, one per channel, all sharing the
//! same marker and frame index (channel `c`'s payload is its own 16 DSD
//! bits). A DAC strips the markers and recovers the original per-channel
//! DSD bitstream at the rate implied by the PCM sample rate:
//!
//! | DSD rate    | DoP rate  | label |
//! |-------------|-----------|-------|
//! | 2_822_400   | 176_400   | dop64 |
//! | 5_644_800   | 352_800   | dop128|
//! | 11_289_600  | 705_600   | dop256|
//!
//! (DoP rate = DSD rate / 16, one PCM sample per 16 DSD bits.)
//!
//! The stream is a 44-byte PCM WAV: format tag 1 (PCM), 24-bit samples,
//! actual sample rate and channel count, real RIFF/data chunk sizes computed
//! from the source DSD payload. Payload is packed on the fly in bounded
//! chunks — memory stays O(chunk), independent of file size.
//!
//! Seek: `?seek_ms=` lands on the DoP frame boundary at or before the
//! requested time (floor), i.e. the first DSD bit index is `frame * 16`
//! per channel — a whole multiple of 16 DSD bits per channel.

use std::io::{Read, Seek};

use axum::body::Body;
use bytes::Bytes;
use kahawai_core::{AudioFormat, MusicError};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::dsd::{DsdBitReader, DsdInfo};

// ---------------------------------------------------------------------------
// DoP framing constants
// ---------------------------------------------------------------------------

/// Number of DSD bits carried in one DoP frame per channel.
pub const DOP_BITS_PER_FRAME: u64 = 16;
/// Bytes per DoP sample: 24-bit PCM.
pub const DOP_BYTES_PER_SAMPLE: usize = 3;
/// DoP markers alternate strictly 0x05 (even frames) / 0xFA (odd frames).
pub const DOP_MARKER_EVEN: u8 = 0x05;
pub const DOP_MARKER_ODD: u8 = 0xFA;
/// Size of the PCM WAV header we emit (RIFF + fmt + data chunks).
pub const WAV_HEADER_LEN: usize = 44;

/// Number of DSD bits a single WAV data byte carries per channel —
/// implied by 16 payload bits / 3 bytes.
const fn dop_rate_for(dsd_rate: u32) -> Option<u32> {
    match dsd_rate {
        2_822_400 => Some(176_400),
        5_644_800 => Some(352_800),
        11_289_600 => Some(705_600),
        _ => None,
    }
}

/// Short chain label for a DSD rate, e.g. `2_822_400` → `"dop64"`.
pub fn dop_chain_label(dsd_rate: u32) -> &'static str {
    match dsd_rate {
        2_822_400 => "dop64",
        5_644_800 => "dop128",
        11_289_600 => "dop256",
        _ => "dop?",
    }
}

// ---------------------------------------------------------------------------
// DoP plan
// ---------------------------------------------------------------------------

/// Everything the DoP streamer needs, resolved up front from the source
/// headers. WAV sizes are real: the payload has a whole number of DoP
/// frames, so the RIFF and data chunk lengths are known without packing.
#[derive(Debug, Clone, Copy)]
pub struct DopPlan {
    /// DoP (PCM) sample rate: 176.4 / 352.8 / 705.6 kHz.
    pub dop_rate: u32,
    /// Channel count from the DSD source.
    pub channels: usize,
    /// Whole DoP frames per channel (floor of usable bits / 16).
    pub frames: u64,
    /// WAV data chunk size in bytes: frames * channels * 3.
    pub data_len: u64,
    /// Total stream length in bytes, including the 44-byte header.
    pub total_len: u64,
    /// Source DSD rate (for chain labels).
    pub dsd_rate: u32,
    /// Was the source a DSF container (vs DFF)?
    pub is_dsf: bool,
}

impl DopPlan {
    /// Resolve a DoP plan from an open DSD source. `format` must be
    /// [`AudioFormat::Dsf`] or [`AudioFormat::Dff`]; anything else is a
    /// clear 4xx (`BadRequest`).
    pub fn resolve<R: Read + Seek>(
        inner: R,
        format: AudioFormat,
    ) -> Result<(Self, DsdBitReader<R>), MusicError> {
        if !matches!(format, AudioFormat::Dsf | AudioFormat::Dff) {
            return Err(MusicError::BadRequest(format!(
                "format=dop requires a DSD (DSF/DFF) source, got {format:?}"
            )));
        }
        let reader = DsdBitReader::open(inner)?;
        let info: &DsdInfo = reader.info();
        let dop_rate = dop_rate_for(info.dsd_rate).ok_or_else(|| {
            MusicError::UnsupportedFormat(format!("unsupported DSD rate {}", info.dsd_rate))
        })?;
        // Usable bits per channel = whole bytes; frames are whole 16-bit
        // groups, so a final partial frame is dropped from the stream.
        let frames = reader.total_bits() / DOP_BITS_PER_FRAME;
        let data_len = frames * info.channels as u64 * DOP_BYTES_PER_SAMPLE as u64;
        Ok((
            Self {
                dop_rate,
                channels: info.channels,
                frames,
                data_len,
                total_len: data_len + WAV_HEADER_LEN as u64,
                dsd_rate: info.dsd_rate,
                is_dsf: info.is_dsf,
            },
            reader,
        ))
    }

    /// WAV header for this plan: 44-byte PCM WAV, format tag 1, 24-bit
    /// samples, real chunk sizes.
    pub fn wav_header(&self) -> [u8; WAV_HEADER_LEN] {
        let channels = self.channels as u16;
        let byte_rate = self.dop_rate * channels as u32 * DOP_BYTES_PER_SAMPLE as u32;
        let block_align = channels * DOP_BYTES_PER_SAMPLE as u16;
        let riff_len = (self.total_len - 8) as u32;
        let data_len = self.data_len as u32;

        let mut h = [0u8; WAV_HEADER_LEN];
        h[0..4].copy_from_slice(b"RIFF");
        h[4..8].copy_from_slice(&riff_len.to_le_bytes());
        h[8..12].copy_from_slice(b"WAVE");
        h[12..16].copy_from_slice(b"fmt ");
        h[16..20].copy_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        h[20..22].copy_from_slice(&1u16.to_le_bytes()); // format tag: PCM
        h[22..24].copy_from_slice(&channels.to_le_bytes());
        h[24..28].copy_from_slice(&self.dop_rate.to_le_bytes());
        h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        h[32..34].copy_from_slice(&block_align.to_le_bytes());
        h[34..36].copy_from_slice(&24u16.to_le_bytes()); // bits per sample
        h[36..40].copy_from_slice(b"data");
        h[40..44].copy_from_slice(&data_len.to_le_bytes());
        h
    }

    /// DoP frame index for a `seek_ms` request: the frame whose time is at
    /// or before the requested time (floor). One frame = 16 DSD bits =
    /// `dop_rate`-relative sample time.
    pub fn seek_frame(&self, seek_ms: u64) -> u64 {
        (seek_ms * self.dop_rate as u64 / 1000).min(self.frames)
    }

    /// Source DSD bit offset (per channel) for a DoP frame index.
    pub fn frame_bit_offset(frame: u64) -> u64 {
        frame * DOP_BITS_PER_FRAME
    }

    /// Stream byte length when starting at DoP frame `frame`. Frame 0 is
    /// the whole stream including the WAV header; a non-zero frame skips
    /// the header (the seeked stream is the suffix of the full pack).
    pub fn seeked_len(&self, frame: u64) -> u64 {
        if frame == 0 {
            return self.total_len;
        }
        let frame = frame.min(self.frames);
        self.total_len
            - WAV_HEADER_LEN as u64
            - frame * self.channels as u64 * DOP_BYTES_PER_SAMPLE as u64
    }

    /// Chain value for `X-Transcode-Chain`, e.g. `dsf64->dop64`.
    pub fn chain_label(&self) -> String {
        let src = if self.is_dsf { "dsf" } else { "dff" };
        let mult = match self.dsd_rate {
            2_822_400 => "64",
            5_644_800 => "128",
            11_289_600 => "256",
            _ => "?",
        };
        format!("{src}{mult}->{}", dop_chain_label(self.dsd_rate))
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Bounded DoP packer: emits the 44-byte WAV header, then packs DSD into
/// DoP frames in bounded chunks. Memory is O(chunk), independent of file
/// size. After `seek_frame(f)`, the first packed frame carries marker
/// `f`-parity and DSD bits starting at `f * 16` per channel.
pub struct DopStreamer<R: Read + Seek> {
    reader: DsdBitReader<R>,
    plan: DopPlan,
    /// Next DoP frame index to pack.
    frame: u64,
    /// Header not yet emitted (some of the 44 bytes may remain).
    header_left: [u8; WAV_HEADER_LEN],
    header_pos: usize,
    /// Packed payload chunk buffer: whole DoP frames.
    chunk: Vec<u8>,
    chunk_pos: usize,
    /// Scratch: whole frames' worth of DSD words (channel multiple of u16).
    words: Vec<u16>,
    /// Marker parity must continue across chunks / seeks.
    marker_parity: u8,
}

impl<R: Read + Seek> DopStreamer<R> {
    /// Number of whole DoP frames packed per payload chunk.
    pub const FRAMES_PER_CHUNK: usize = 4096;

    /// Create a streamer for `plan`, starting at the beginning (frame 0).
    /// Use [`seek_frame`](Self::seek_frame) before first `fill` for seeks.
    pub fn new(reader: DsdBitReader<R>, plan: DopPlan) -> Self {
        let words = vec![0u16; Self::FRAMES_PER_CHUNK * plan.channels];
        Self {
            reader,
            plan,
            frame: 0,
            header_left: plan.wav_header(),
            header_pos: 0,
            chunk: Vec::new(),
            chunk_pos: 0,
            words,
            marker_parity: 0,
        }
    }

    /// Start the stream at DoP frame `frame` (from
    /// [`DopPlan::seek_frame`]): positions the bit reader at DSD bit
    /// `frame * 16` per channel. Bit-exact: seeking to frame `f` yields
    /// exactly the packed frames `f..` of a full pack. A non-zero frame
    /// skips the WAV header (the seeked stream is the suffix of the full
    /// pack starting at that frame); frame 0 keeps the header.
    pub fn seek_frame(&mut self, frame: u64) -> Result<(), MusicError> {
        let frame = frame.min(self.plan.frames);
        self.reader.seek_bit(DopPlan::frame_bit_offset(frame))?;
        self.frame = frame;
        self.marker_parity = (frame % 2) as u8;
        if frame > 0 {
            self.header_pos = WAV_HEADER_LEN; // header already "consumed"
        }
        self.chunk.clear();
        self.chunk_pos = 0;
        Ok(())
    }

    /// Fill `out` with the next stream bytes (header first, then packed
    /// DoP). Returns bytes written; 0 at end of stream. Each call packs at
    /// most one bounded chunk.
    pub fn fill(&mut self, out: &mut [u8]) -> Result<usize, MusicError> {
        let mut written = 0;

        // 1. WAV header.
        if self.header_pos < WAV_HEADER_LEN {
            let n = (WAV_HEADER_LEN - self.header_pos).min(out.len() - written);
            out[written..written + n]
                .copy_from_slice(&self.header_left[self.header_pos..self.header_pos + n]);
            self.header_pos += n;
            written += n;
            if written == out.len() {
                return Ok(written);
            }
        }

        // 2. Packed DoP payload, one bounded chunk at a time.
        while written < out.len() {
            if self.chunk_pos >= self.chunk.len() && !self.pack_chunk()? {
                break; // end of stream
            }
            let n = (self.chunk.len() - self.chunk_pos).min(out.len() - written);
            out[written..written + n]
                .copy_from_slice(&self.chunk[self.chunk_pos..self.chunk_pos + n]);
            self.chunk_pos += n;
            written += n;
        }
        Ok(written)
    }

    /// Pack the next chunk of whole DoP frames. Returns false at end of
    /// stream (no more frames).
    fn pack_chunk(&mut self) -> Result<bool, MusicError> {
        let ch = self.plan.channels;
        let want_frames = (Self::FRAMES_PER_CHUNK as u64).min(self.plan.frames - self.frame);
        if want_frames == 0 {
            return Ok(false);
        }
        let want_words = want_frames as usize * ch;
        let got = self.reader.read_words(&mut self.words[..want_words])?;
        let got_frames = (got / ch) as u64;
        if got_frames == 0 {
            return Ok(false);
        }
        self.chunk.clear();
        self.chunk
            .reserve(got_frames as usize * ch * DOP_BYTES_PER_SAMPLE);
        for f in 0..got_frames as usize {
            let marker = if (self.marker_parity as usize + f).is_multiple_of(2) {
                DOP_MARKER_EVEN
            } else {
                DOP_MARKER_ODD
            };
            for c in 0..ch {
                let w = self.words[f * ch + c];
                // 24-bit little-endian sample: payload in the low 16 bits,
                // marker in the most significant byte.
                self.chunk.push((w & 0xff) as u8);
                self.chunk.push((w >> 8) as u8);
                self.chunk.push(marker);
            }
        }
        self.chunk_pos = 0;
        self.frame += got_frames;
        self.marker_parity = ((self.marker_parity as u64 + got_frames) % 2) as u8;
        Ok(true)
    }
}

/// Chained DoP response body for one or more tracks (S8 `?next=`): each
/// track's DoP stream — complete with its own WAV header — is packed in
/// turn by a single blocking thread, mirroring
/// [`crate::transcode::transcode_body`]. Packing is cheap bit shuffling;
/// the thread exists to keep blocking file IO off the async runtime.
///
/// Best-effort by design: a client that treats the response as one
/// continuous byte stream will hit the second WAV header mid-stream.
/// Documented here rather than hidden; DoP gapless has its own client
/// audio path anyway. The response carries `X-Gapless-Mode: chained`.
pub fn dop_body_chained<R: Read + Seek + Send + 'static>(
    mut streamers: Vec<DopStreamer<R>>,
) -> Body {
    let (tx, rx) = mpsc::channel::<Result<Bytes, MusicError>>(32);
    tokio::task::spawn_blocking(move || {
        let mut buf = vec![0u8; 64 * 1024];
        for streamer in &mut streamers {
            loop {
                match streamer.fill(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx
                            .blocking_send(Ok(Bytes::copy_from_slice(&buf[..n])))
                            .is_err()
                        {
                            return; // client went away
                        }
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return;
                    }
                }
            }
        }
    });
    Body::from_stream(ReceiverStream::new(rx))
}

/// Test fixtures shared with the HTTP integration tests in `main.rs`.
/// Minimal valid DSF / DFF files carrying exact caller-supplied payloads,
/// validated by the real parser via [`DsdBitReader`].
#[cfg(test)]
pub(crate) mod fixture {
    /// DSF block length used by the fixtures (Sony spec §II).
    pub const BLOCK_LEN: usize = 4096;

    pub fn make_dsf(
        channels: usize,
        dsd_rate: u32,
        ch_payloads: &[Vec<u8>],
        samples_per_channel: u64,
    ) -> Vec<u8> {
        assert_eq!(ch_payloads.len(), channels);
        let mut v = Vec::new();
        v.extend_from_slice(b"DSD ");
        v.extend_from_slice(&28u64.to_le_bytes());
        let file_size_pos = v.len();
        v.extend_from_slice(&0u64.to_le_bytes());
        let data_ptr_pos = v.len();
        v.extend_from_slice(&0u64.to_le_bytes());
        // fmt chunk: 52 bytes total per Sony DSF 1.01 §II.
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&52u64.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes()); // version
        v.extend_from_slice(&0u32.to_le_bytes()); // format id: uncompressed
        let channel_type: u32 = match channels {
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => 6,
            _ => 5,
        };
        v.extend_from_slice(&channel_type.to_le_bytes());
        v.extend_from_slice(&(channels as u32).to_le_bytes());
        v.extend_from_slice(&dsd_rate.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes()); // bits per sample
        v.extend_from_slice(&samples_per_channel.to_le_bytes());
        v.extend_from_slice(&(BLOCK_LEN as u32).to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // reserved
                                                  // data chunk: block groups interleaved — for each block, every
                                                  // channel's block bytes (Sony DSF 1.01 §II).
        let data_off = v.len();
        v.extend_from_slice(b"data");
        let blocks = ch_payloads[0].len() / BLOCK_LEN;
        let mut payload = Vec::with_capacity(ch_payloads[0].len() * channels);
        for b in 0..blocks {
            for ch in ch_payloads {
                payload.extend_from_slice(&ch[b * BLOCK_LEN..(b + 1) * BLOCK_LEN]);
            }
        }
        v.extend_from_slice(&(12u64 + payload.len() as u64).to_le_bytes());
        v.extend_from_slice(&payload);
        let total = v.len() as u64;
        v[file_size_pos..file_size_pos + 8].copy_from_slice(&total.to_le_bytes());
        v[data_ptr_pos..data_ptr_pos + 8].copy_from_slice(&(data_off as u64).to_le_bytes());
        v
    }

    pub fn make_dff(channels: usize, dsd_rate: u32, frames_payload: &[u8]) -> Vec<u8> {
        assert!(frames_payload.len().is_multiple_of(channels));
        let mut v = Vec::new();
        v.extend_from_slice(b"FRM8");
        let form_size_pos = v.len();
        v.extend_from_slice(&0u64.to_be_bytes()); // patched later
        v.extend_from_slice(b"DSD ");
        v.extend_from_slice(b"FVER");
        v.extend_from_slice(&4u64.to_be_bytes());
        v.extend_from_slice(&0x0105_0000u32.to_be_bytes());
        // PROP > SND > (FS, CHNL, CMPR)
        let mut prop = Vec::new();
        prop.extend_from_slice(b"SND ");
        prop.extend_from_slice(b"FS  ");
        prop.extend_from_slice(&4u64.to_be_bytes());
        prop.extend_from_slice(&dsd_rate.to_be_bytes());
        prop.extend_from_slice(b"CHNL");
        prop.extend_from_slice(&(2u64 + channels as u64 * 4).to_be_bytes());
        prop.extend_from_slice(&(channels as u16).to_be_bytes());
        prop.extend_from_slice(&vec![0u8; channels * 4]); // channel IDs
        prop.extend_from_slice(b"CMPR");
        prop.extend_from_slice(&4u64.to_be_bytes());
        prop.extend_from_slice(b"DSD ");
        v.extend_from_slice(b"PROP");
        v.extend_from_slice(&(prop.len() as u64).to_be_bytes());
        v.extend_from_slice(&prop);
        // DSD chunk: frame-interleaved payload.
        v.extend_from_slice(b"DSD ");
        v.extend_from_slice(&(frames_payload.len() as u64).to_be_bytes());
        v.extend_from_slice(frames_payload);
        let total = v.len() as u64;
        v[form_size_pos..form_size_pos + 8].copy_from_slice(&(total - 12).to_be_bytes());
        v
    }

    /// Pseudo-random (non-symmetric) bit pattern from a simple LCG, so
    /// DC-symmetric shortcuts can't hide bit-order bugs.
    pub fn lcg_bits(seed: u64, n: usize) -> Vec<bool> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) & 1 == 1
            })
            .collect()
    }

    /// Write `bits` (time order, per channel) into a DSF-style payload
    /// (LSB-first bytes, channel blocks) and a DFF-style payload
    /// (MSB-first bytes, frame-interleaved), each wrapped in a minimal
    /// valid container.
    pub fn pair_fixtures(channels: usize, dsd_rate: u32, bits: &[bool]) -> (Vec<u8>, Vec<u8>) {
        let per_ch = bits.len() / channels;
        assert!(bits.len().is_multiple_of(channels));
        let bl = BLOCK_LEN;
        // DSF: LSB-first within each byte; channel blocks of `bl` bytes.
        let nbytes = per_ch.div_ceil(8);
        let blocks = nbytes.div_ceil(bl);
        let mut ch_payloads: Vec<Vec<u8>> = (0..channels).map(|_| vec![0u8; blocks * bl]).collect();
        for i in 0..per_ch {
            for c in 0..channels {
                if bits[i * channels + c] {
                    ch_payloads[c][i / 8] |= 1 << (i % 8);
                }
            }
        }
        let dsf = make_dsf(channels, dsd_rate, &ch_payloads, per_ch as u64);
        // DFF: MSB-first within each byte; frame = one byte per channel.
        let mut dff_payload = Vec::with_capacity(nbytes * channels);
        for i in 0..nbytes {
            for c in 0..channels {
                let mut byte = 0u8;
                for k in 0..8 {
                    let bit_idx = i * 8 + k;
                    let b = bit_idx < per_ch && bits[bit_idx * channels + c];
                    byte |= (b as u8) << (7 - k);
                }
                dff_payload.push(byte);
            }
        }
        let dff = make_dff(channels, dsd_rate, &dff_payload);
        (dsf, dff)
    }

    /// Build a standalone DSF fixture (stereo unless noted) carrying
    /// `bits` (time order, channel-interleaved). Bit count per channel is
    /// padded up to whole DSF block groups with zeros.
    pub fn dsf_fixture(channels: usize, dsd_rate: u32, bits: &[bool]) -> Vec<u8> {
        pair_fixtures(channels, dsd_rate, bits).0
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::dsd::DsdBitReader;
    use std::io::Cursor;

    /// Pack a whole fixture to DoP bytes via DopStreamer.
    fn pack_all(dsd: &[u8], format: AudioFormat) -> (DopPlan, Vec<u8>) {
        let (plan, reader) = DopPlan::resolve(Cursor::new(dsd.to_vec()), format).unwrap();
        let mut s = DopStreamer::new(reader, plan);
        let mut out = Vec::with_capacity(plan.total_len as usize);
        let mut buf = [0u8; 8192];
        loop {
            let n = s.fill(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        (plan, out)
    }

    /// Test unpacker: inverse of the packer — verifies marker alternation
    /// and recovers the payload bits in time order (MSB oldest).
    fn unpack_dop(stream: &[u8], plan: &DopPlan) -> (Vec<Vec<bool>>, bool) {
        let data = &stream[WAV_HEADER_LEN..];
        assert_eq!(data.len() as u64, plan.data_len);
        let mut ch_bits: Vec<Vec<bool>> = (0..plan.channels).map(|_| Vec::new()).collect();
        let mut markers_ok = true;
        for (f, frame) in data
            .chunks(plan.channels * DOP_BYTES_PER_SAMPLE)
            .enumerate()
        {
            let want = if f % 2 == 0 {
                DOP_MARKER_EVEN
            } else {
                DOP_MARKER_ODD
            };
            for c in 0..plan.channels {
                let s = &frame[c * 3..c * 3 + 3];
                if s[2] != want {
                    markers_ok = false;
                }
                let w = u16::from_le_bytes([s[0], s[1]]);
                for k in (0..16).rev() {
                    ch_bits[c].push((w >> k) & 1 == 1);
                }
            }
        }
        (ch_bits, markers_ok)
    }

    #[test]
    fn dop64_stereo_marker_alternation_and_bit_exact_roundtrip() {
        // Bit count is a multiple of one DSF block group (32768 bits) so
        // no block padding inflates the frame count.
        let ch = 2;
        let per_ch = 65_536;
        let bits = lcg_bits(0x1234_5678_9abc_def0, ch * per_ch);
        let (dsf, _) = pair_fixtures(ch, 2_822_400, &bits);
        let (plan, stream) = pack_all(&dsf, AudioFormat::Dsf);
        assert_eq!(plan.dop_rate, 176_400);
        assert_eq!(plan.channels, 2);
        assert_eq!(plan.frames, per_ch as u64 / 16);
        assert_eq!(plan.data_len, plan.frames * 2 * 3);
        assert_eq!(stream.len() as u64, plan.total_len);

        let (recovered, markers_ok) = unpack_dop(&stream, &plan);
        assert!(markers_ok, "markers must alternate 0x05/0xFA strictly");
        for c in 0..ch {
            let want: Vec<bool> = bits.chunks(ch).map(|s| s[c]).collect();
            assert_eq!(recovered[c], want, "channel {c} payload not bit-exact");
        }
    }

    #[test]
    fn dop_dsf_dff_identical_payload() {
        let ch = 2;
        let per_ch = 32_768;
        let bits = lcg_bits(0xdead_beef_cafe_f00d, ch * per_ch);
        let (dsf, dff) = pair_fixtures(ch, 2_822_400, &bits);
        let (plan_s, stream_s) = pack_all(&dsf, AudioFormat::Dsf);
        let (plan_f, stream_f) = pack_all(&dff, AudioFormat::Dff);
        assert_eq!(plan_s.frames, per_ch as u64 / 16);
        assert_eq!(plan_s.frames, plan_f.frames);
        // Identical time-order bitstream -> identical DoP payload (headers
        // agree too since sizes match).
        assert_eq!(
            stream_s, stream_f,
            "DSF and DFF must produce identical DoP streams"
        );
    }

    #[test]
    fn dop_rates_per_dsd_rate() {
        let per_ch = 32_768; // one DSF block group: no padding
        for (dsd_rate, dop_rate) in [
            (2_822_400, 176_400),
            (5_644_800, 352_800),
            (11_289_600, 705_600),
        ] {
            let bits = lcg_bits(dsd_rate as u64, 2 * per_ch);
            let (dsf, _) = pair_fixtures(2, dsd_rate, &bits);
            let (plan, stream) = pack_all(&dsf, AudioFormat::Dsf);
            assert_eq!(plan.dop_rate, dop_rate);
            assert_eq!(plan.frames, per_ch as u64 / 16);
            // WAV header sanity: format tag PCM, 24-bit, rate, channels.
            let h = &stream[..WAV_HEADER_LEN];
            assert_eq!(&h[0..4], b"RIFF");
            assert_eq!(&h[8..12], b"WAVE");
            assert_eq!(&h[12..16], b"fmt ");
            assert_eq!(u16::from_le_bytes([h[20], h[21]]), 1, "format tag PCM");
            assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2, "channels");
            assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), dop_rate);
            assert_eq!(u16::from_le_bytes([h[34], h[35]]), 24, "bits per sample");
            assert_eq!(&h[36..40], b"data");
            let riff_len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64;
            assert_eq!(riff_len + 8, stream.len() as u64, "real RIFF size");
            let data_len = u32::from_le_bytes([h[40], h[41], h[42], h[43]]) as u64;
            assert_eq!(data_len, plan.data_len, "real data size");
            let byte_rate = u32::from_le_bytes([h[28], h[29], h[30], h[31]]);
            assert_eq!(byte_rate, dop_rate * 2 * 3);
        }
    }

    #[test]
    fn dop_multichannel_layout() {
        // 6 channels: every channel packed in the same frame, channel c at
        // byte offset c*3 within the frame, same marker across channels.
        let ch = 6;
        let per_ch = 32_768; // one DSF block group: no padding
        let frames = per_ch / 16;
        let bits = lcg_bits(0xfeed_face_1234_5678, ch * per_ch);
        let (dsf, _) = pair_fixtures(ch, 2_822_400, &bits);
        let (plan, stream) = pack_all(&dsf, AudioFormat::Dsf);
        assert_eq!(plan.frames, frames as u64);
        let data = &stream[WAV_HEADER_LEN..];
        for (f, frame) in data.chunks(ch * DOP_BYTES_PER_SAMPLE).enumerate() {
            let want = if f % 2 == 0 {
                DOP_MARKER_EVEN
            } else {
                DOP_MARKER_ODD
            };
            for c in 0..ch {
                let s = &frame[c * 3..c * 3 + 3];
                assert_eq!(s[2], want, "frame {f} channel {c} marker");
                let w = u16::from_le_bytes([s[0], s[1]]);
                let mut want_w = 0u16;
                for k in 0..16 {
                    let b = bits[(f * 16 + k) * ch + c];
                    want_w |= (b as u16) << (15 - k);
                }
                assert_eq!(w, want_w, "frame {f} channel {c} payload");
            }
        }
    }

    #[test]
    fn dsd_bit_reader_seek_is_bit_exact() {
        let ch = 2;
        let per_ch = 32_768;
        let bits = lcg_bits(0xabc1_23ff, ch * per_ch);
        let (dsf, _) = pair_fixtures(ch, 2_822_400, &bits);
        let mut r = DsdBitReader::open(Cursor::new(dsf)).unwrap();
        assert_eq!(r.total_bits(), per_ch as u64);
        assert_eq!(r.bits_read(), 0);
        // Read 3 frames = 48 bits per channel.
        let mut words = [0u16; 2 * 3];
        assert_eq!(r.read_words(&mut words).unwrap(), 6);
        assert_eq!(r.bits_read(), 48);
        // Seek backwards: re-reading yields identical words.
        r.seek_bit(16).unwrap();
        assert_eq!(r.bits_read(), 16);
        let mut words2 = [0u16; 2 * 2];
        assert_eq!(r.read_words(&mut words2).unwrap(), 4);
        assert_eq!(&words2, &words[2..6]);
        // Seek forward past the current position.
        r.seek_bit(160).unwrap();
        assert_eq!(r.bits_read(), 160);
    }

    #[test]
    fn dop_seek_equals_slice_of_full_pack() {
        let ch = 2;
        // DSD128: 352.8 DoP frames per ms — pick seek_ms values and derive
        // the expected frame floor from them.
        let per_ch = 131_072; // 4 DSF block groups: no padding
        let bits = lcg_bits(0x0dd_c0de_5eed_beef, ch * per_ch);
        let (dsf, _) = pair_fixtures(ch, 5_644_800, &bits);
        let (plan, full) = pack_all(&dsf, AudioFormat::Dsf);
        assert_eq!(plan.frames, per_ch as u64 / 16); // 8192 frames ≈ 23 ms

        for &seek_ms in &[0u64, 1, 5, 10, 20] {
            let frame = plan.seek_frame(seek_ms);
            assert_eq!(frame, seek_ms * 352_800 / 1000);
            assert!(frame < plan.frames);
            let (plan2, reader) =
                DopPlan::resolve(Cursor::new(dsf.clone()), AudioFormat::Dsf).unwrap();
            let mut s = DopStreamer::new(reader, plan2);
            s.seek_frame(frame).unwrap();
            let mut out = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = s.fill(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
            assert_eq!(out.len() as u64, plan2.seeked_len(frame));
            // Seeked stream = the full pack sliced at the target frame
            // (frame 0 keeps the WAV header, so the slice starts at 0).
            let off = if frame == 0 {
                0
            } else {
                WAV_HEADER_LEN + frame as usize * ch * DOP_BYTES_PER_SAMPLE
            };
            assert_eq!(
                out,
                full[off..],
                "seek_ms={seek_ms} (frame {frame}) must equal slicing the full pack"
            );
        }
    }

    #[test]
    fn dop_partial_frame_dropped() {
        // 165 bits per channel = 10 whole DoP frames + 5 trailing bits.
        // The trailing 5 bits don't fill a DoP frame and must not appear in
        // the stream. Uses the DFF fixture (no block padding).
        let ch = 2;
        let per_ch = 16 * 10 + 5;
        let bits = lcg_bits(0x5eed_5eed_5eed_5eed, ch * per_ch);
        let (_, dff) = pair_fixtures(ch, 2_822_400, &bits);
        let (plan, stream) = pack_all(&dff, AudioFormat::Dff);
        assert_eq!(plan.frames, 10);
        assert_eq!(stream.len() as u64, plan.total_len);
        let (recovered, markers_ok) = unpack_dop(&stream, &plan);
        assert!(markers_ok);
        assert_eq!(recovered[0].len(), 10 * 16);
        // And the 160 kept bits are exactly the first 160 source bits.
        for c in 0..ch {
            let want: Vec<bool> = bits.chunks(ch).map(|s| s[c]).take(160).collect();
            assert_eq!(recovered[c], want);
        }
    }

    #[test]
    fn dop_rejects_non_dsd_source() {
        let err = DopPlan::resolve(Cursor::new(vec![0u8; 64]), AudioFormat::Flac)
            .map(|_| ())
            .expect_err("non-DSD source must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("DSD"),
            "error must name the DSD requirement, got: {msg}"
        );
    }

    #[test]
    fn dop_chain_labels() {
        let bits = lcg_bits(42, 2 * 16 * 8);
        for (dsd_rate, want) in [
            (2_822_400u32, "dsf64->dop64"),
            (5_644_800u32, "dsf128->dop128"),
            (11_289_600u32, "dsf256->dop256"),
        ] {
            let (dsf, dff) = pair_fixtures(2, dsd_rate, &bits);
            let (plan, _) = pack_all(&dsf, AudioFormat::Dsf);
            assert_eq!(plan.chain_label(), want);
            let (plan_f, _) = pack_all(&dff, AudioFormat::Dff);
            assert_eq!(plan_f.chain_label(), want.replace("dsf", "dff"));
        }
    }

    /// S8: chained DoP emits complete independent WAV streams back to back.
    /// Best-effort gapless — each stream restarts its markers at 0x05 and
    /// its payload is bit-exact, so a DoP-aware client can re-lock on each
    /// WAV header. A client that stops at the first header gets one track
    /// plus the X-Gapless-Next hint (honest degradation).
    #[tokio::test]
    async fn dop_body_chained_two_complete_streams() {
        let ch = 2;
        let per_ch = 32_768;
        let bit_sets = [
            lcg_bits(0x1111_2222_3333_4444, ch * per_ch),
            lcg_bits(0x5555_6666_7777_8888, ch * per_ch),
        ];
        let mut streamers = Vec::new();
        let mut plans = Vec::new();
        for bits in &bit_sets {
            let dsf = dsf_fixture(ch, 2_822_400, bits);
            let (plan, reader) = DopPlan::resolve(Cursor::new(dsf), AudioFormat::Dsf).unwrap();
            streamers.push(DopStreamer::new(reader, plan));
            plans.push(plan);
        }
        let body = dop_body_chained(streamers);
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();

        // Two complete WAV streams, back to back.
        let riffs: Vec<usize> = bytes
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == b"RIFF")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(riffs.len(), 2, "chained DoP must contain two WAV streams");
        assert_eq!(riffs[0], 0);
        let one_len = plans[0].total_len as usize;
        assert_eq!(one_len, 44 + 2048 * ch * 3);
        assert_eq!(
            riffs[1], one_len,
            "second stream starts right after the first"
        );
        assert_eq!(bytes.len(), 2 * one_len);

        // Each stream is independently valid: markers restart at 0x05 and
        // the payload is bit-exact against its own source bits.
        for (i, bits) in bit_sets.iter().enumerate() {
            let stream = &bytes[i * one_len..(i + 1) * one_len];
            let (recovered, markers_ok) = unpack_dop(stream, &plans[i]);
            assert!(markers_ok, "stream {i}: markers must restart and alternate");
            for c in 0..ch {
                let want: Vec<bool> = bits.chunks(ch).map(|s| s[c]).collect();
                assert_eq!(recovered[c], want, "stream {i} ch{c} not bit-exact");
            }
        }
    }
}
