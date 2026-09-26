//! DoP (DSD-over-PCM) client stream handling. (Spec §2, S5b.)
//!
//! The server sends DoP as 24-bit little-endian PCM frames in a WAV
//! container (`?format=dop`): DSD64 → 176.4 kHz, DSD128 → 352.8 kHz,
//! DSD256 → 705.6 kHz, low 16 bits DSD payload, high byte alternating
//! `0x05`/`0xFA`.
//!
//! Two response shapes exist (server Phase-4 behavior, test-pinned):
//! 1. **Fresh play**: a 44-byte WAV header, then the frames.
//! 2. **Seeked** (`?seek_ms=`): the raw payload suffix with **no** header.
//!    The client then continues with the format established by the pre-seek
//!    session. The server guarantees marker (0x05/0xFA) parity across
//!    seeks, so the client feeds the bytes through untouched — never
//!    re-aligning, never touching a single bit (no volume, no EQ, no
//!    resample, no dither).
//!
//! Chained gapless (`X-Gapless-Mode: chained` + `?next=`) concatenates one
//! WAV per track; [`DopStream`] re-parses the header at each segment
//! boundary.

use std::io::{Cursor, Read};

use kahawai_core::MusicError;

/// DoP is always 24-bit PCM on the wire.
pub const DOP_BITS_PER_SAMPLE: u8 = 24;

/// PCM rate the DoP stream needs for a DSD source rate, or `None` for an
/// unknown DSD rate. DoP packs 16 DSD bits per PCM sample: ÷16.
pub fn dop_pcm_rate(dsd_rate_hz: u32) -> Option<u32> {
    match dsd_rate_hz {
        2_822_400 => Some(176_400),  // DSD64
        5_644_800 => Some(352_800),  // DSD128
        11_289_600 => Some(705_600), // DSD256
        _ => None,
    }
}

/// Validated DoP stream parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DopSpec {
    /// Source DSD rate, e.g. 2_822_400.
    pub dsd_rate_hz: u32,
    /// PCM rate on the wire, e.g. 176_400.
    pub dop_rate_hz: u32,
    pub channels: u8,
    /// Payload bytes from the WAV `data` chunk; `None` for raw seek
    /// continuations (length unknown, stream to EOF).
    pub data_bytes: Option<u64>,
}

impl DopSpec {
    /// Bytes per DoP frame (one 24-bit sample per channel).
    pub fn frame_bytes(&self) -> usize {
        self.channels as usize * 3
    }
}

fn u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

/// Parse and validate the 44-byte WAV header the server sends for
/// `?format=dop`. `expected_dop_rate` comes from the catalog DSD rate via
/// [`dop_pcm_rate`] — the header must agree, or the DAC would play garbage.
pub fn parse_wav_header(hdr: &[u8; 44], expected_dop_rate: u32) -> Result<DopSpec, MusicError> {
    if &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"WAVE" {
        return Err(MusicError::BadRequest("DoP response is not a WAV".into()));
    }
    if &hdr[12..16] != b"fmt " {
        return Err(MusicError::BadRequest("DoP WAV has no fmt chunk".into()));
    }
    if u32_le(&hdr[16..20]) != 16 {
        return Err(MusicError::BadRequest(
            "DoP WAV fmt chunk is not PCM".into(),
        ));
    }
    if u16_le(&hdr[20..22]) != 1 {
        return Err(MusicError::BadRequest("DoP WAV is not integer PCM".into()));
    }
    let channels = u16_le(&hdr[22..24]);
    if !(1..=8).contains(&channels) {
        return Err(MusicError::BadRequest(format!(
            "DoP WAV channel count out of range: {channels}"
        )));
    }
    let rate = u32_le(&hdr[24..28]);
    if rate != expected_dop_rate {
        return Err(MusicError::BadRequest(format!(
            "DoP WAV rate {rate} != expected {expected_dop_rate}"
        )));
    }
    let bits = u16_le(&hdr[34..36]);
    if bits != DOP_BITS_PER_SAMPLE as u16 {
        return Err(MusicError::BadRequest(format!(
            "DoP WAV is {bits}-bit, expected 24-bit"
        )));
    }
    if u32_le(&hdr[28..32]) != rate * channels as u32 * 3 {
        return Err(MusicError::BadRequest("DoP WAV byte rate mismatch".into()));
    }
    if u16_le(&hdr[32..34]) != channels * 3 {
        return Err(MusicError::BadRequest(
            "DoP WAV block align mismatch".into(),
        ));
    }
    if &hdr[36..40] != b"data" {
        return Err(MusicError::BadRequest("DoP WAV has no data chunk".into()));
    }
    let data_bytes = u32_le(&hdr[40..44]) as u64;
    if !data_bytes.is_multiple_of(channels as u64 * 3) {
        return Err(MusicError::BadRequest(
            "DoP WAV data length is not a whole number of frames".into(),
        ));
    }
    Ok(DopSpec {
        dsd_rate_hz: expected_dop_rate * 16,
        dop_rate_hz: expected_dop_rate,
        channels: channels as u8,
        data_bytes: Some(data_bytes),
    })
}

/// Byte stream of DoP frames for one engine playback.
///
/// Constructed from the raw response body: when it starts with `RIFF` the
/// header is parsed and validated; otherwise the body is a seek
/// continuation and `established` (the pre-seek session's spec) applies.
/// In chained mode each segment carries its own header, re-parsed at the
/// boundary. Reads are always whole DoP frames.
pub struct DopStream {
    reader: Box<dyn Read + Send>,
    /// Sniffed bytes not yet consumed (the non-`RIFF` prefix of a raw
    /// seek continuation).
    prefix: Option<Cursor<Vec<u8>>>,
    spec: DopSpec,
    /// Bytes left in the current WAV data chunk; `None` = raw mode.
    remaining: Option<u64>,
    chained: bool,
    done: bool,
    /// Stashed bytes that don't yet make a whole frame. The seek prefix
    /// (frame-unaligned by construction) merges here on first read, and
    /// only complete DoP frames ever leave `read_frames`; a truncated
    /// tail is dropped when the stream ends.
    carry: Vec<u8>,
    /// WAV headers successfully parsed (1 = the first). Lets the engine
    /// count how many chained segments actually played.
    pub segments_completed: usize,
}

impl DopStream {
    pub fn new(
        mut reader: Box<dyn Read + Send>,
        expected_dop_rate: u32,
        established: Option<DopSpec>,
        chained: bool,
    ) -> Result<Self, MusicError> {
        let mut sniff = [0u8; 4];
        reader.read_exact(&mut sniff).map_err(MusicError::Io)?;
        if &sniff == b"RIFF" {
            let mut hdr = [0u8; 44];
            hdr[..4].copy_from_slice(&sniff);
            reader.read_exact(&mut hdr[4..]).map_err(MusicError::Io)?;
            let spec = parse_wav_header(&hdr, expected_dop_rate)?;
            let remaining = spec.data_bytes;
            Ok(Self {
                reader,
                prefix: None,
                spec,
                remaining,
                chained,
                done: false,
                carry: Vec::new(),
                segments_completed: 1,
            })
        } else {
            let spec = established.ok_or_else(|| {
                MusicError::BadRequest(
                    "seeked DoP response has no WAV header and no established format".into(),
                )
            })?;
            // The 4 sniffed bytes are payload — serve them first.
            Ok(Self {
                reader,
                prefix: Some(Cursor::new(sniff.to_vec())),
                spec,
                remaining: None,
                chained: false, // raw continuations never chain
                done: false,
                carry: Vec::new(),
                segments_completed: 1,
            })
        }
    }

    pub fn spec(&self) -> DopSpec {
        self.spec
    }

    fn fill_from_reader(&mut self, buf: &mut [u8]) -> Result<usize, MusicError> {
        let mut total = 0;
        while total < buf.len() {
            match self.reader.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) => return Err(MusicError::Io(e)),
            }
        }
        Ok(total)
    }

    /// Advance past the current segment: in chained mode expect another
    /// `RIFF` header; otherwise the stream is done.
    fn next_segment(&mut self) -> Result<(), MusicError> {
        if !self.chained {
            self.done = true;
            return Ok(());
        }
        let mut sniff = [0u8; 4];
        match self.reader.read_exact(&mut sniff) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.done = true;
                return Ok(());
            }
            Err(e) => return Err(MusicError::Io(e)),
        }
        if &sniff != b"RIFF" {
            // Not a header where one was expected: stop cleanly; the
            // engine falls back to sequential requests.
            self.done = true;
            return Ok(());
        }
        let mut hdr = [0u8; 44];
        hdr[..4].copy_from_slice(&sniff);
        self.reader
            .read_exact(&mut hdr[4..])
            .map_err(MusicError::Io)?;
        let spec = parse_wav_header(&hdr, self.spec.dop_rate_hz)?;
        if spec.channels != self.spec.channels {
            return Err(MusicError::BadRequest(
                "chained DoP segment changed channel count".into(),
            ));
        }
        self.remaining = spec.data_bytes;
        self.segments_completed += 1;
        Ok(())
    }

    /// Read DoP payload into `buf`, always a whole number of frames
    /// (`n % frame_bytes() == 0` unless `n == 0`). The seek-continuation
    /// prefix is frame-*un*aligned by construction (it starts
    /// mid-marker-parity), so it folds into the carry buffer and only
    /// complete frames are ever returned. Returns 0 at end of stream.
    pub fn read_frames(&mut self, buf: &mut [u8]) -> Result<usize, MusicError> {
        if self.done {
            return Ok(0);
        }
        let frame = self.spec.frame_bytes();
        if buf.len() < frame {
            return Ok(0);
        }

        // Seek-continuation prefix: at most 4 bytes sitting in memory;
        // fold them into the carry so alignment is handled once, below.
        if let Some(prefix) = self.prefix.take() {
            let mut head = prefix.into_inner();
            head.extend_from_slice(&self.carry);
            self.carry = head;
        }

        // Serve stashed bytes first.
        let mut out = 0usize;
        if !self.carry.is_empty() {
            let n = self.carry.len().min(buf.len());
            buf[..n].copy_from_slice(&self.carry[..n]);
            self.carry.drain(..n);
            out = n;
        }

        // Pull whole frames from the reader; short reads accumulate.
        let want = (buf.len() - out) / frame * frame;
        while out < want {
            if let Some(rem) = self.remaining {
                if rem == 0 {
                    self.next_segment()?;
                    if self.done {
                        break;
                    }
                    continue;
                }
            }
            let cap = want - out;
            let cap = match self.remaining {
                Some(rem) => cap.min(rem as usize),
                None => cap,
            };
            // Keep frame granularity even against `remaining`.
            let cap = cap / frame * frame;
            if cap == 0 {
                break;
            }
            // Accumulate raw bytes; frame alignment is enforced once,
            // on return, so short reads never lose bytes.
            let n = self.fill_from_reader(&mut buf[out..out + cap])?;
            if n == 0 {
                // EOF: stop cleanly; any stashed partial frame is dropped.
                self.done = true;
                break;
            }
            out += n;
            if let Some(rem) = self.remaining.as_mut() {
                *rem -= n as u64;
            }
        }

        // Never emit a partial DoP frame: a truncated tail stays stashed
        // (and is dropped when the stream ends) rather than corrupting
        // the marker stream the DAC decodes.
        let whole = out / frame * frame;
        if whole < out {
            self.carry.extend_from_slice(&buf[whole..out]);
        }
        Ok(whole)
    }

    pub fn is_done(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 24-bit WAV with an alternating 0x05/0xFA marker pattern.
    fn dop_wav(dop_rate: u32, channels: u16, frames: usize, marker_start: u8) -> Vec<u8> {
        let data_len = frames * channels as usize * 3;
        let mut v = Vec::with_capacity(44 + data_len);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&dop_rate.to_le_bytes());
        v.extend_from_slice(&(dop_rate * channels as u32 * 3).to_le_bytes());
        v.extend_from_slice(&(channels * 3).to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data_len as u32).to_le_bytes());
        let mut marker = marker_start;
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

    fn read_all(mut s: DopStream) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 777]; // deliberately not frame-aligned
        loop {
            let n = s.read_frames(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            // Whole frames, always.
            assert_eq!(n % s.spec().frame_bytes(), 0);
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    #[test]
    fn rate_mapping() {
        assert_eq!(dop_pcm_rate(2_822_400), Some(176_400));
        assert_eq!(dop_pcm_rate(5_644_800), Some(352_800));
        assert_eq!(dop_pcm_rate(11_289_600), Some(705_600));
        assert_eq!(dop_pcm_rate(44100), None);
        assert_eq!(dop_pcm_rate(0), None);
    }

    #[test]
    fn header_parses_and_validates() {
        let wav = dop_wav(176_400, 2, 100, 0x05);
        let mut hdr = [0u8; 44];
        hdr.copy_from_slice(&wav[..44]);
        let spec = parse_wav_header(&hdr, 176_400).unwrap();
        assert_eq!(
            spec,
            DopSpec {
                dsd_rate_hz: 2_822_400,
                dop_rate_hz: 176_400,
                channels: 2,
                data_bytes: Some(600),
            }
        );
        // Wrong expected rate is refused.
        assert!(parse_wav_header(&hdr, 352_800).is_err());
        // Corrupt the bits field.
        let mut bad = hdr;
        bad[34] = 16;
        assert!(parse_wav_header(&bad, 176_400).is_err());
    }

    #[test]
    fn fresh_play_strips_header_feeds_frames_untouched() {
        let wav = dop_wav(176_400, 2, 64, 0x05);
        let payload = wav[44..].to_vec();
        let s = DopStream::new(Box::new(Cursor::new(wav)), 176_400, None, false).unwrap();
        assert_eq!(s.spec().channels, 2);
        let out = read_all(s);
        assert_eq!(out, payload, "DoP bytes must pass through bit-identical");
        // Marker parity survived: every frame's high byte alternates.
        for (i, frame) in out.as_chunks::<6>().0.iter().enumerate() {
            let expect = if i % 2 == 0 { 0x05 } else { 0xFA };
            assert_eq!(frame[2], expect, "frame {i} marker");
            assert_eq!(frame[5], expect, "frame {i} marker ch2");
        }
    }

    #[test]
    fn seek_continuation_uses_established_format() {
        let established = DopSpec {
            dsd_rate_hz: 2_822_400,
            dop_rate_hz: 176_400,
            channels: 2,
            data_bytes: None,
        };
        // Raw payload suffix starting mid-marker-parity (0xFA first):
        // the server guarantees parity; the client must not re-align.
        let raw = dop_wav(176_400, 2, 32, 0xFA)[44..].to_vec();
        let first4 = raw[..4].to_vec();
        assert_ne!(&first4, b"RIFF");
        let s = DopStream::new(
            Box::new(Cursor::new(raw.clone())),
            176_400,
            Some(established),
            false,
        )
        .unwrap();
        let out = read_all(s);
        assert_eq!(out, raw);
        assert_eq!(out[2], 0xFA, "parity continues from the seek point");
    }

    #[test]
    fn seek_without_established_format_fails() {
        let raw = vec![0xAA, 0x55, 0x05, 0xAA, 0x55, 0xFA];
        let err = match DopStream::new(Box::new(Cursor::new(raw)), 176_400, None, false) {
            Ok(_) => panic!("expected failure"),
            Err(e) => e,
        };
        assert!(matches!(err, MusicError::BadRequest(_)));
    }

    #[test]
    fn chained_segments_reparse_headers() {
        let a = dop_wav(176_400, 2, 32, 0x05);
        let b = dop_wav(176_400, 2, 16, 0x05);
        let mut body = a.clone();
        body.extend_from_slice(&b);
        let s = DopStream::new(Box::new(Cursor::new(body)), 176_400, None, true).unwrap();
        let out = read_all(s);
        let mut expect = a[44..].to_vec();
        expect.extend_from_slice(&b[44..]);
        assert_eq!(out, expect);
    }

    #[test]
    fn chained_stops_cleanly_on_garbage() {
        let a = dop_wav(176_400, 2, 32, 0x05);
        let mut body = a.clone();
        body.extend_from_slice(b"XXXX trailing junk");
        let s = DopStream::new(Box::new(Cursor::new(body)), 176_400, None, true).unwrap();
        let out = read_all(s);
        assert_eq!(out, a[44..].to_vec());
    }
}
