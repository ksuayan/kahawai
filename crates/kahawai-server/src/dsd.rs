//! In-house DSD (DSF/DFF) parsing and FIR decimation DSD → PCM.
//! (Spec §2, S5a.)
//!
//! No mature Rust DSD decoder exists, so this module does it directly:
//!
//! - **DSF** (Sony): `DSD ` chunk, `fmt ` chunk (little-endian), `data` chunk.
//!   Payload is *block-interleaved*: each block holds `block_len` bytes for
//!   channel 0, then `block_len` bytes for channel 1, and so on. Each byte
//!   packs 8 DSD bits, LSB first (Sony DSF spec v1.01 §II, Annotation 4:
//!   for bits_per_sample = 1 the data is stored "LSB first"); bit 1 = positive.
//! - **DFF** (Philips DSDIFF): `FRM8` container (big-endian), `PROP`/`SND`
//!   sub-chunks for the sample rate (`FS `), channel count (`CHNL`) and
//!   compression (`CMPR` — only uncompressed `DSD ` is accepted; `DST`
//!   is rejected), and a `DSD ` chunk whose payload is *frame-interleaved*:
//!   each frame is one byte per channel in channel order.
//!
//! Only DSD64 (2.8224 MHz), DSD128 (5.6448 MHz) and DSD256 (11.2896 MHz) at
//! 1–6 channels are accepted; anything else is a hard error, not a guess.
//!
//! ## Decimator design (documented, honest)
//!
//! Single-stage polyphase-in-spirit FIR decimator (implemented as a direct-
//! form FIR evaluated once per output sample):
//!
//! - DSD64 ÷ 32 → 88.2 kHz, DSD128 ÷ 32 → 176.4 kHz, DSD256 ÷ 64 → 176.4 kHz.
//!   Integer ratios landing on standard PCM rates, so no second resampling
//!   stage is needed before FLAC encoding.
//! - Kaiser-windowed sinc low-pass: 20 kHz passband edge, stopband from
//!   0.95 × (PCM rate / 2), 75 dB stopband attenuation. DSD carries heavy
//!   shaped quantization noise above ~30 kHz; the stopband is placed to
//!   kill it, not just to satisfy Nyquist.
//! - DSD bit 1 → +1.0, bit 0 → −1.0. A 50 %-density bitstream (digital
//!   silence) therefore decodes to 0.0 — no DC offset by construction.
//! - Output is f32; the transcode stage scales to 24-bit for FLAC.
//!
//! This is v1 quality, not a mastering-grade modulator: a single-stage FIR
//! with ~600 taps costs ~100 M MAC/s for stereo DSD64 — trivially real-time
//! on one core — but a serious DSD DAC would use a multi-stage design with
//! better ultrasonic rejection. The filter actually used is documented in
//! [`Decimator::describe`].

use std::io::{Read, Seek, SeekFrom};

use kahawai_core::MusicError;

// ---------------------------------------------------------------------------
// Format constants
// ---------------------------------------------------------------------------

/// Accepted DSD sample rates: DSD64 / DSD128 / DSD256.
const DSD_RATES: [u32; 3] = [2_822_400, 5_644_800, 11_289_600];

/// PCM rate produced for each DSD rate (integer decimation ratios).
pub fn pcm_rate_for_dsd(dsd_rate: u32) -> Option<u32> {
    match dsd_rate {
        2_822_400 => Some(88_200),   // ÷32
        5_644_800 => Some(176_400),  // ÷32
        11_289_600 => Some(176_400), // ÷64
        _ => None,
    }
}

/// Short chain label for the `X-Transcode-Chain` header, e.g. `dsf64`.
pub fn dsd_chain_label(dsd_rate: u32, is_dsf: bool) -> &'static str {
    match (is_dsf, dsd_rate) {
        (true, 2_822_400) => "dsf64",
        (true, 5_644_800) => "dsf128",
        (true, 11_289_600) => "dsf256",
        (false, 2_822_400) => "dff64",
        (false, 5_644_800) => "dff128",
        (false, 11_289_600) => "dff256",
        _ => "dsd",
    }
}

// ---------------------------------------------------------------------------
// Parsed stream info
// ---------------------------------------------------------------------------

/// How the DSD payload bytes are laid out in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadLayout {
    /// DSF: repeating groups of `channels × block_len` bytes; within a
    /// group, channel `c` owns bytes `[c*block_len, (c+1)*block_len)`.
    Blocked,
    /// DFF: repeating frames of `channels` bytes; frame `f`, channel `c`
    /// is at `f*channels + c`.
    FrameInterleaved,
}

/// Validated DSD stream description, independent of container.
#[derive(Debug, Clone)]
pub struct DsdInfo {
    pub channels: usize,
    pub dsd_rate: u32,
    /// DSD samples (bits) per channel.
    #[allow(dead_code)]
    pub samples_per_channel: u64,
    /// File offset of the first DSD payload byte.
    pub payload_offset: u64,
    /// Usable payload length in bytes (whole groups/frames only).
    pub payload_len: u64,
    pub layout: PayloadLayout,
    /// DSF only: payload bytes per channel per block.
    pub block_len: usize,
    /// True for DSF, false for DFF.
    pub is_dsf: bool,
}

impl DsdInfo {
    /// PCM sample rate this stream decimates to.
    pub fn pcm_rate(&self) -> u32 {
        pcm_rate_for_dsd(self.dsd_rate).expect("validated at parse time")
    }

    /// Integer decimation ratio.
    pub fn ratio(&self) -> usize {
        (self.dsd_rate / self.pcm_rate()) as usize
    }

    /// Total PCM samples per channel the stream yields.
    #[allow(dead_code)]
    pub fn total_pcm_samples(&self) -> u64 {
        self.samples_per_channel / self.ratio() as u64
    }
}

// ---------------------------------------------------------------------------
// Low-level readers
// ---------------------------------------------------------------------------

fn read_u32_le<R: Read>(r: &mut R) -> Result<u32, MusicError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).map_err(MusicError::Io)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64_le<R: Read>(r: &mut R) -> Result<u64, MusicError> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).map_err(MusicError::Io)?;
    Ok(u64::from_le_bytes(b))
}

fn read_u32_be<R: Read>(r: &mut R) -> Result<u32, MusicError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).map_err(MusicError::Io)?;
    Ok(u32::from_be_bytes(b))
}

fn read_u64_be<R: Read>(r: &mut R) -> Result<u64, MusicError> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).map_err(MusicError::Io)?;
    Ok(u64::from_be_bytes(b))
}

fn read_chunk_id<R: Read>(r: &mut R) -> Result<[u8; 4], MusicError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).map_err(MusicError::Io)?;
    Ok(b)
}

fn bad(msg: impl Into<String>) -> MusicError {
    MusicError::BadRequest(format!("DSD parse: {}", msg.into()))
}

fn check_dsd_rate(rate: u32) -> Result<(), MusicError> {
    if DSD_RATES.contains(&rate) {
        Ok(())
    } else {
        Err(bad(format!(
            "unsupported DSD sample rate {rate} (want one of {DSD_RATES:?})"
        )))
    }
}

fn check_channels(ch: u64) -> Result<usize, MusicError> {
    if (1..=6).contains(&ch) {
        Ok(ch as usize)
    } else {
        Err(bad(format!("unsupported channel count {ch}")))
    }
}

// ---------------------------------------------------------------------------
// DSF parsing (Sony, little-endian)
// ---------------------------------------------------------------------------

/// Parse a DSF stream. `r` is positioned at the start of the file.
fn parse_dsf<R: Read + Seek>(r: &mut R) -> Result<DsdInfo, MusicError> {
    if &read_chunk_id(r)? != b"DSD " {
        return Err(bad("missing 'DSD ' magic"));
    }
    let chunk_size = read_u64_le(r)?;
    if chunk_size != 28 {
        return Err(bad(format!("DSD chunk size {chunk_size}, want 28")));
    }
    let _file_size = read_u64_le(r)?;
    let _data_ptr = read_u64_le(r)?;

    let mut channels = None;
    let mut dsd_rate = None;
    let mut sample_count = None;
    let mut block_len = None;
    let mut payload_offset = None;
    let mut payload_len = 0u64;

    // Walk chunks by ID; tolerate (skip) anything unrecognized.
    while let Ok(id) = read_chunk_id(r) {
        let size = read_u64_le(r)?;
        if size < 12 {
            return Err(bad(format!("chunk size {size} < 12")));
        }
        match &id {
            b"fmt " => {
                // Sony DSF spec v1.01 §II: the fmt chunk is 52 bytes total
                // (12-byte header + 40 bytes of fields, ending with the
                // 4-byte reserved field). Accept trailing bytes beyond 52
                // for forward compatibility.
                if size < 52 {
                    return Err(bad(format!("fmt chunk size {size}, want >= 52")));
                }
                let version = read_u32_le(r)?;
                let format_id = read_u32_le(r)?;
                let _channel_type = read_u32_le(r)?;
                let ch = read_u32_le(r)? as u64;
                let rate = read_u32_le(r)?;
                let bits = read_u32_le(r)?;
                let count = read_u64_le(r)?;
                let blk = read_u32_le(r)?;
                let _reserved = read_u32_le(r)?;
                if size > 52 {
                    r.seek(SeekFrom::Current((size - 52) as i64))
                        .map_err(MusicError::Io)?;
                }
                if version != 1 {
                    return Err(bad(format!("fmt version {version}, want 1")));
                }
                if format_id != 0 {
                    return Err(bad("compressed DSF (format id != 0) not supported"));
                }
                if bits != 1 {
                    return Err(bad(format!("bits per sample {bits}, want 1")));
                }
                check_dsd_rate(rate)?;
                let ch = check_channels(ch)?;
                if blk == 0 {
                    return Err(bad("block size 0"));
                }
                if count == 0 {
                    return Err(bad("sample count 0"));
                }
                channels = Some(ch);
                dsd_rate = Some(rate);
                sample_count = Some(count);
                block_len = Some(blk as usize);
            }
            b"data" => {
                payload_offset = Some(r.stream_position().map_err(MusicError::Io)?);
                payload_len = size - 12;
                break; // payload is the last thing we need
            }
            _ => {
                // Skip unknown chunk payload.
                let skip = size - 12;
                r.seek(SeekFrom::Current(skip as i64))
                    .map_err(MusicError::Io)?;
            }
        }
    }

    let (channels, dsd_rate, sample_count, block_len, payload_offset) =
        match (channels, dsd_rate, sample_count, block_len, payload_offset) {
            (Some(c), Some(r), Some(n), Some(b), Some(o)) => (c, r, n, b, o),
            _ => return Err(bad("missing fmt or data chunk")),
        };

    // Whole block groups only; a trailing partial group is ignored.
    let group = channels as u64 * block_len as u64;
    let payload_len = payload_len / group * group;

    Ok(DsdInfo {
        channels,
        dsd_rate,
        samples_per_channel: sample_count,
        payload_offset,
        payload_len,
        layout: PayloadLayout::Blocked,
        block_len,
        is_dsf: true,
    })
}

// ---------------------------------------------------------------------------
// DFF parsing (Philips DSDIFF, big-endian)
// ---------------------------------------------------------------------------

/// Parse a DFF stream. `r` is positioned at the start of the file.
fn parse_dff<R: Read + Seek>(r: &mut R) -> Result<DsdInfo, MusicError> {
    if &read_chunk_id(r)? != b"FRM8" {
        return Err(bad("missing 'FRM8' magic"));
    }
    let _form_size = read_u64_be(r)?;
    if &read_chunk_id(r)? != b"DSD " {
        return Err(bad("FRM8 form type is not 'DSD '"));
    }

    let mut dsd_rate = None;
    let mut channels = None;
    let mut payload_offset = None;
    let mut payload_len = 0u64;

    while let Ok(id) = read_chunk_id(r) {
        // DSDIFF 1.5: ckDataSize is the size of the *data portion* of the
        // chunk — it does NOT include the 12 bytes of ckID + ckDataSize.
        let data_len = read_u64_be(r)?;
        match &id {
            b"FVER" => {
                let v = read_u32_be(r)?;
                if v != 0x0105_0000 {
                    return Err(bad(format!("DFF version {v:#x}, want 1.5")));
                }
                if data_len > 4 {
                    r.seek(SeekFrom::Current((data_len - 4) as i64))
                        .map_err(MusicError::Io)?;
                }
            }
            b"PROP" => {
                // PROP payload: "SND " then sub-chunks in the same format.
                if &read_chunk_id(r)? != b"SND " {
                    return Err(bad("PROP chunk is not SND"));
                }
                let mut left = data_len - 4;
                while left >= 12 {
                    let sub = read_chunk_id(r)?;
                    let sub_data = read_u64_be(r)?;
                    if sub_data > left - 12 {
                        return Err(bad("malformed PROP sub-chunk"));
                    }
                    match &sub {
                        b"FS  " => {
                            dsd_rate = Some(read_u32_be(r)?);
                            if sub_data > 4 {
                                r.seek(SeekFrom::Current((sub_data - 4) as i64))
                                    .map_err(MusicError::Io)?;
                            }
                        }
                        b"CHNL" => {
                            let mut nbuf = [0u8; 2];
                            r.read_exact(&mut nbuf).map_err(MusicError::Io)?;
                            channels = Some(check_channels(u16::from_be_bytes(nbuf) as u64)?);
                            if sub_data > 2 {
                                r.seek(SeekFrom::Current((sub_data - 2) as i64))
                                    .map_err(MusicError::Io)?;
                            }
                        }
                        b"CMPR" => {
                            let cmp = read_chunk_id(r)?;
                            if &cmp == b"DST " {
                                return Err(MusicError::UnsupportedFormat(
                                    "DST-compressed DFF is not supported in v1 (spec §2: v2)"
                                        .into(),
                                ));
                            }
                            if &cmp != b"DSD " {
                                return Err(bad(format!(
                                    "unknown DFF compression '{}'",
                                    String::from_utf8_lossy(&cmp)
                                )));
                            }
                            if sub_data > 4 {
                                r.seek(SeekFrom::Current((sub_data - 4) as i64))
                                    .map_err(MusicError::Io)?;
                            }
                        }
                        _ => {
                            r.seek(SeekFrom::Current(sub_data as i64))
                                .map_err(MusicError::Io)?;
                        }
                    }
                    left -= 12 + sub_data;
                }
            }
            b"DSD " => {
                payload_offset = Some(r.stream_position().map_err(MusicError::Io)?);
                payload_len = data_len;
                break;
            }
            _ => {
                r.seek(SeekFrom::Current(data_len as i64))
                    .map_err(MusicError::Io)?;
            }
        }
    }

    let (channels, dsd_rate, payload_offset) = match (channels, dsd_rate, payload_offset) {
        (Some(c), Some(r), Some(o)) => (c, r, o),
        _ => return Err(bad("missing FS/CHNL PROP or DSD chunk")),
    };
    check_dsd_rate(dsd_rate)?;

    // Whole frames only.
    let payload_len = payload_len / channels as u64 * channels as u64;
    let samples_per_channel = payload_len / channels as u64 * 8;

    Ok(DsdInfo {
        channels,
        dsd_rate,
        samples_per_channel,
        payload_offset,
        payload_len,
        layout: PayloadLayout::FrameInterleaved,
        block_len: 1,
        is_dsf: false,
    })
}

/// Sniff the container and parse. `r` must be at the start of the file.
pub fn parse_dsd<R: Read + Seek>(r: &mut R) -> Result<DsdInfo, MusicError> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).map_err(MusicError::Io)?;
    r.seek(SeekFrom::Start(0)).map_err(MusicError::Io)?;
    match &magic {
        b"DSD " => parse_dsf(r),
        b"FRM8" => parse_dff(r),
        _ => Err(bad(format!(
            "not a DSD file (magic '{}')",
            String::from_utf8_lossy(&magic)
        ))),
    }
}

// ---------------------------------------------------------------------------
// Kaiser FIR design
// ---------------------------------------------------------------------------

/// Modified Bessel function I₀, Abramowitz & Stegun 9.8.1 polynomial
/// approximations. Good to ~1e-7 relative — plenty for a window function.
fn bessel_i0(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 3.75 {
        let y = (x / 3.75).powi(2);
        1.0 + y
            * (3.5156229
                + y * (3.0899424
                    + y * (1.2067492 + y * (0.2659732 + y * (0.0360768 + y * 0.0045813)))))
    } else {
        let y = 3.75 / ax;
        (ax.exp() / ax.sqrt())
            * (0.39894228
                + y * (0.01328592
                    + y * (0.00225319
                        + y * (-0.00157565
                            + y * (0.00916281
                                + y * (-0.02057706
                                    + y * (0.02635537 + y * (-0.01647633 + y * 0.00392377))))))))
    }
}

/// Design the decimation low-pass: Kaiser-windowed sinc.
///
/// * passband edge 20 kHz, stopband from 0.95 × (pcm_rate/2), 75 dB.
/// * returns taps normalized to DC gain 1.0.
fn design_filter(dsd_rate: u32, pcm_rate: u32) -> Vec<f32> {
    const PASS_EDGE: f64 = 20_000.0;
    const ATTEN_DB: f64 = 75.0;

    let stop_edge = pcm_rate as f64 / 2.0 * 0.95;
    let cutoff = (PASS_EDGE + stop_edge) / 2.0 / dsd_rate as f64; // cycles/sample
    let trans_width = (stop_edge - PASS_EDGE) / dsd_rate as f64; // cycles/sample

    // Kaiser beta for the target attenuation.
    let beta = 0.1102 * (ATTEN_DB - 8.7);
    // Kaiser order estimate: N ≈ (A − 8) / (2.285 · Δω), Δω in rad/sample.
    let delta_omega = 2.0 * std::f64::consts::PI * trans_width;
    let mut n = ((ATTEN_DB - 8.0) / (2.285 * delta_omega)).ceil() as usize;
    n = n.clamp(64, 2048);
    if n.is_multiple_of(2) {
        n += 1; // odd length → symmetric, integer group delay
    }

    let m = (n - 1) as f64 / 2.0;
    let i0_beta = bessel_i0(beta);
    let mut taps = Vec::with_capacity(n);
    for i in 0..n {
        let x = i as f64 - m;
        // Sinc, scaled so the peak is 1.0.
        let sinc = if x.abs() < 1e-9 {
            1.0
        } else {
            let u = 2.0 * cutoff * x;
            (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u)
        };
        // Kaiser window.
        let w_arg = beta * (1.0 - (x / m).powi(2)).max(0.0).sqrt();
        let w = bessel_i0(w_arg) / i0_beta;
        taps.push((2.0 * cutoff * sinc * w) as f32);
    }
    // Normalize to DC gain 1.0.
    let sum: f32 = taps.iter().sum();
    for t in taps.iter_mut() {
        *t /= sum;
    }
    taps
}

// ---------------------------------------------------------------------------
// Decimator
// ---------------------------------------------------------------------------

/// One channel of the decimation filter: direct-form FIR evaluated once per
/// output sample, with a ring-buffer delay line.
struct ChannelDecimator {
    coeffs: Vec<f32>,
    hist: Vec<f32>,
    /// Next write position in `hist`.
    pos: usize,
    /// Input samples pushed since the last output.
    count: usize,
    ratio: usize,
}

impl ChannelDecimator {
    fn new(coeffs: &[f32], ratio: usize) -> Self {
        Self {
            coeffs: coeffs.to_vec(),
            hist: vec![0.0; coeffs.len()],
            pos: 0,
            count: 0,
            ratio,
        }
    }

    /// Push one DSD sample (+1.0 / −1.0). Returns an output PCM sample every
    /// `ratio` pushes.
    fn push(&mut self, x: f32) -> Option<f32> {
        let n = self.hist.len();
        self.hist[self.pos] = x;
        self.pos = (self.pos + 1) % n;
        self.count += 1;
        if self.count < self.ratio {
            return None;
        }
        self.count = 0;
        // y = Σ h[k]·x[n−k]; x[n] (newest) sits at (pos−1) mod n.
        let mut acc = 0.0f32;
        for (k, h) in self.coeffs.iter().enumerate() {
            let idx = (self.pos + n - 1 - k) % n;
            acc += h * self.hist[idx];
        }
        Some(acc)
    }

    fn reset(&mut self) {
        self.hist.fill(0.0);
        self.pos = 0;
        self.count = 0;
    }
}

/// DSD bits of one payload byte in time order (index 0 = earliest sample).
/// Bit order differs by container (verified against the format specs):
/// - DSF: LSB-first — Sony DSF spec v1.01 §II, Annotation 4: with
///   bits_per_sample = 1 the data is stored "LSB first" (e.g. stream value
///   0x01 is stored as byte 0x80).
/// - DFF: MSB-first — DSDIFF 1.5 §3.4.3: the DSD stream is numbered
///   "starting with the most significant bit of the first byte".
fn dsd_byte_time_order(byte: u8, is_dsf: bool) -> [bool; 8] {
    let mut out = [false; 8];
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = if is_dsf { i as u8 } else { 7 - i as u8 };
        *slot = (byte >> shift) & 1 == 1;
    }
    out
}

/// DSD → PCM decimator for one stream: one [`ChannelDecimator`] per channel
/// sharing a single prototype filter.
pub struct Decimator {
    channels: Vec<ChannelDecimator>,
    ratio: usize,
    taps: usize,
    dsd_rate: u32,
    pcm_rate: u32,
}

impl Decimator {
    pub fn new(dsd_rate: u32, channels: usize) -> Result<Self, MusicError> {
        let pcm_rate = pcm_rate_for_dsd(dsd_rate)
            .ok_or_else(|| bad(format!("unsupported DSD rate {dsd_rate}")))?;
        let ratio = (dsd_rate / pcm_rate) as usize;
        let coeffs = design_filter(dsd_rate, pcm_rate);
        let taps = coeffs.len();
        Ok(Self {
            channels: (0..channels)
                .map(|_| ChannelDecimator::new(&coeffs, ratio))
                .collect(),
            ratio,
            taps,
            dsd_rate,
            pcm_rate,
        })
    }

    #[allow(dead_code)]
    pub fn channels(&self) -> usize {
        self.channels.len()
    }
    #[allow(dead_code)]
    pub fn ratio(&self) -> usize {
        self.ratio
    }
    pub fn taps(&self) -> usize {
        self.taps
    }
    pub fn pcm_rate(&self) -> u32 {
        self.pcm_rate
    }

    /// Human-readable filter description for logs.
    /// (Also exercised by unit tests; kept as public API.)
    #[allow(dead_code)]
    pub fn describe(&self) -> String {
        let label = match self.dsd_rate {
            2_822_400 => "DSD64",
            5_644_800 => "DSD128",
            11_289_600 => "DSD256",
            _ => "DSD",
        };
        format!(
            "{label} → PCM: ÷{} to {} Hz, {}-tap Kaiser FIR (20 kHz pass, 75 dB stop)",
            self.ratio, self.pcm_rate, self.taps,
        )
    }

    /// Push one DSD bit per channel (`true` = +1.0). Returns `Some(frame)`
    /// every `ratio` pushes — one f32 PCM sample per channel.
    pub fn push_bits(&mut self, bits: &[bool], out: &mut [f32]) -> bool {
        debug_assert_eq!(bits.len(), self.channels.len());
        debug_assert_eq!(out.len(), self.channels.len());
        let mut ready = false;
        for (c, dec) in self.channels.iter_mut().enumerate() {
            let x = if bits[c] { 1.0 } else { -1.0 };
            if let Some(y) = dec.push(x) {
                out[c] = y;
                ready = true;
            }
        }
        ready
    }

    pub fn reset(&mut self) {
        for c in self.channels.iter_mut() {
            c.reset();
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming DSD → PCM reader
// ---------------------------------------------------------------------------

/// Raw DSD payload walker: turns the container-specific byte layout (DSF
/// block groups / DFF frame interleave) into time-ordered bits per channel.
/// Shared by the PCM decimator ([`DsdPcmReader`]) and the bit-level reader
/// ([`DsdBitReader`], used by the DoP packer) so the tricky layout code
/// exists exactly once.
struct PayloadWalker<R: Read + Seek> {
    inner: R,
    info: DsdInfo,
    /// Bytes of payload not yet pulled into `group`.
    payload_left: u64,
    /// Current raw payload group.
    group: Vec<u8>,
    /// Read cursor within `group`, in *channel bytes* (see `next_step`).
    group_cursor: usize,
    /// Channel bytes making up one step, expanded to time-ordered bits at
    /// load time (see [`dsd_byte_time_order`]): Blocked → one byte per
    /// channel from the same block position; Interleaved → one frame.
    step: Vec<[bool; 8]>,
    step_bit: u8, // 0..8, index into the time-ordered bits
    step_valid: bool,
    /// DSD bits consumed per channel (position for seeking).
    bits_read: u64,
}

impl<R: Read + Seek> PayloadWalker<R> {
    /// Open a DSD stream positioned at its start; sniffs DSF vs DFF and
    /// seeks to the first payload byte.
    fn open(mut inner: R) -> Result<Self, MusicError> {
        let info = parse_dsd(&mut inner)?;
        inner
            .seek(SeekFrom::Start(info.payload_offset))
            .map_err(MusicError::Io)?;
        let step = vec![[false; 8]; info.channels];
        Ok(Self {
            inner,
            payload_left: info.payload_len,
            info,
            group: Vec::new(),
            group_cursor: 0,
            step,
            step_bit: 8, // force load on first use
            step_valid: false,
            bits_read: 0,
        })
    }

    fn info(&self) -> &DsdInfo {
        &self.info
    }

    /// Pull the next raw payload group into `self.group`.
    /// Returns false when the payload is exhausted.
    fn fill_group(&mut self) -> Result<bool, MusicError> {
        if self.payload_left == 0 {
            return Ok(false);
        }
        let want: usize = match self.info.layout {
            // One full block group: channels × block_len.
            PayloadLayout::Blocked => self.info.channels * self.info.block_len,
            // 1024 frames: 1024 × channels bytes.
            PayloadLayout::FrameInterleaved => 1024 * self.info.channels,
        };
        let want = (want as u64).min(self.payload_left) as usize;
        self.group.resize(want, 0);
        self.inner
            .read_exact(&mut self.group)
            .map_err(MusicError::Io)?;
        self.payload_left -= want as u64;
        self.group_cursor = 0;
        Ok(true)
    }

    /// Load the next per-channel byte step. Returns false at end of payload.
    fn next_step(&mut self) -> Result<bool, MusicError> {
        let ch = self.info.channels;
        loop {
            let ok = match self.info.layout {
                PayloadLayout::Blocked => {
                    let bl = self.info.block_len;
                    let group_bytes = ch * bl;
                    if self.group_cursor + ch > group_bytes || self.group.len() < group_bytes {
                        None
                    } else {
                        let byte_idx = self.group_cursor;
                        if byte_idx >= bl {
                            None
                        } else {
                            let is_dsf = self.info.is_dsf;
                            for c in 0..ch {
                                self.step[c] =
                                    dsd_byte_time_order(self.group[c * bl + byte_idx], is_dsf);
                            }
                            self.group_cursor += 1;
                            Some(())
                        }
                    }
                }
                PayloadLayout::FrameInterleaved => {
                    if self.group_cursor + ch <= self.group.len() {
                        let is_dsf = self.info.is_dsf;
                        for c in 0..ch {
                            self.step[c] =
                                dsd_byte_time_order(self.group[self.group_cursor + c], is_dsf);
                        }
                        self.group_cursor += ch;
                        Some(())
                    } else {
                        None
                    }
                }
            };
            match ok {
                Some(()) => {
                    self.step_bit = 0;
                    self.step_valid = true;
                    return Ok(true);
                }
                None => {
                    if !self.fill_group()? {
                        self.step_valid = false;
                        return Ok(false);
                    }
                }
            }
        }
    }

    /// Copy the current step's time-ordered bits into `bits[..channels]`.
    fn load_step_bits(&self, bits: &mut [bool]) {
        let ch = self.info.channels;
        for (b, s) in bits.iter_mut().zip(self.step.iter()).take(ch) {
            *b = s[self.step_bit as usize];
        }
    }

    /// Advance the source by `n` DSD bits (per channel) without decoding.
    fn skip_bits(&mut self, mut n: u64) -> Result<(), MusicError> {
        while n > 0 {
            if (!self.step_valid || self.step_bit == 8) && !self.next_step()? {
                break; // past end: nothing more to skip
            }
            let avail = (8 - self.step_bit) as u64;
            let take = avail.min(n);
            self.step_bit += take as u8;
            self.bits_read += take;
            n -= take;
        }
        Ok(())
    }

    /// Rewind to the first payload byte (for backwards seeks).
    fn rewind(&mut self) -> Result<(), MusicError> {
        self.inner
            .seek(SeekFrom::Start(self.info.payload_offset))
            .map_err(MusicError::Io)?;
        self.payload_left = self.info.payload_len;
        self.group.clear();
        self.group_cursor = 0;
        self.step_valid = false;
        self.step_bit = 8;
        self.bits_read = 0;
        Ok(())
    }
}

/// Reads a DSD file and yields interleaved f32 PCM at [`DsdInfo::pcm_rate`].
///
/// Payload is pulled in groups (DSF block groups / DFF frame batches) so
/// memory stays O(group), independent of file size.
pub struct DsdPcmReader<R: Read + Seek> {
    walker: PayloadWalker<R>,
    dec: Decimator,
}

impl<R: Read + Seek> DsdPcmReader<R> {
    /// Open a DSD file positioned at its start; sniffs DSF vs DFF.
    pub fn open(inner: R) -> Result<Self, MusicError> {
        let walker = PayloadWalker::open(inner)?;
        let dec = Decimator::new(walker.info.dsd_rate, walker.info.channels)?;
        Ok(Self { walker, dec })
    }

    pub fn info(&self) -> &DsdInfo {
        self.walker.info()
    }
    pub fn pcm_rate(&self) -> u32 {
        self.dec.pcm_rate()
    }
    pub fn channels(&self) -> usize {
        self.walker.info().channels
    }
    /// FIR tap count of the decimation filter (for logs / diagnostics).
    pub fn taps(&self) -> usize {
        self.dec.taps()
    }

    /// Fill `out` (interleaved f32, len a multiple of channels) with PCM.
    /// Returns the number of f32 samples written (0 at end of stream).
    pub fn read_pcm(&mut self, out: &mut [f32]) -> Result<usize, MusicError> {
        let ch = self.walker.info().channels;
        assert!(
            out.len().is_multiple_of(ch),
            "out len must be a channel multiple"
        );
        let mut written = 0;
        let mut bits = [false; 6];
        let mut frame = [0.0f32; 6];
        while written + ch <= out.len() {
            if (!self.walker.step_valid || self.walker.step_bit == 8) && !self.walker.next_step()? {
                break;
            }
            self.walker.load_step_bits(&mut bits);
            self.walker.step_bit += 1;
            if self.dec.push_bits(&bits[..ch], &mut frame[..ch]) {
                out[written..written + ch].copy_from_slice(&frame[..ch]);
                written += ch;
            }
        }
        Ok(written)
    }

    /// Skip `n` PCM frames. The bulk of the skip is decode-and-drop at the
    /// DSD bit level (no FIR work); then the FIR is warmed with the
    /// `taps - 1` bits immediately preceding the seek point so the first
    /// emitted frame has correct filter history. DSD seek is therefore
    /// sample-exact, matching the Symphonia decode-and-drop path.
    pub fn skip_pcm(&mut self, n: u64) -> Result<(), MusicError> {
        let ratio = self.walker.info().ratio() as u64;
        let target_bits = n * ratio;
        // Warm-up must cover the full N-tap filter history (the ordered
        // last-N inputs are the decimator state) and be a whole number of
        // output frames so the decimator phase (`count`) realigns to 0.
        let warm_target = (self.dec.taps() as u64).div_ceil(ratio) * ratio;
        let warm_bits = warm_target.min(target_bits);
        self.walker.skip_bits(target_bits - warm_bits)?;
        self.dec.reset();
        self.warm_bits(warm_bits)?;
        Ok(())
    }

    /// Push `n` DSD bits through the FIR, discarding output — primes the
    /// filter history so the first frame after a seek is sample-exact.
    fn warm_bits(&mut self, mut n: u64) -> Result<(), MusicError> {
        let ch = self.walker.info().channels;
        let mut bits = [false; 6];
        let mut frame = [0.0f32; 6];
        while n > 0 {
            if (!self.walker.step_valid || self.walker.step_bit == 8) && !self.walker.next_step()? {
                break;
            }
            self.walker.load_step_bits(&mut bits);
            self.walker.step_bit += 1;
            self.dec.push_bits(&bits[..ch], &mut frame[..ch]);
            n -= 1;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bit-level DSD reader (S5b)
// ---------------------------------------------------------------------------

/// Bit-level DSD reader: yields time-ordered DSD bits as packed u16 words
/// (16 bits per word, MSB = oldest bit). Used by the DoP packer.
///
/// Unlike [`DsdPcmReader`] there is no filter state, so seeking is a plain
/// bit-position seek — bit-exact by construction.
pub struct DsdBitReader<R: Read + Seek> {
    walker: PayloadWalker<R>,
}

impl<R: Read + Seek> DsdBitReader<R> {
    /// Open a DSD file positioned at its start; sniffs DSF vs DFF.
    pub fn open(inner: R) -> Result<Self, MusicError> {
        Ok(Self {
            walker: PayloadWalker::open(inner)?,
        })
    }

    pub fn info(&self) -> &DsdInfo {
        self.walker.info()
    }

    /// DSD bits consumed per channel so far.
    #[cfg(test)]
    pub fn bits_read(&self) -> u64 {
        self.walker.bits_read
    }

    /// Usable payload bits per channel (whole bytes).
    pub fn total_bits(&self) -> u64 {
        self.walker.info.payload_len / self.walker.info.channels as u64 * 8
    }

    /// Fill `out` with 16-bit DSD words, frame-major / channel-minor:
    /// `out[0..channels]` is the first 16-bit group for each channel, then
    /// the next group, and so on. Each word holds 16 time-ordered DSD bits,
    /// MSB oldest. `out.len()` must be a channel multiple. Returns the
    /// number of u16 words written; 0 at end of stream. A trailing partial
    /// word (< 16 bits) is dropped — callers size the stream from
    /// `total_bits() / 16` whole frames.
    pub fn read_words(&mut self, out: &mut [u16]) -> Result<usize, MusicError> {
        let ch = self.walker.info.channels;
        assert!(
            out.len().is_multiple_of(ch),
            "out len must be a channel multiple"
        );
        let mut words_written = 0;
        let mut acc = [0u16; 6];
        let mut acc_bits = 0u8;
        while words_written + ch <= out.len() {
            if (!self.walker.step_valid || self.walker.step_bit == 8) && !self.walker.next_step()? {
                break;
            }
            // Shift each new bit in MSB-first: after 16 bits the oldest
            // bit sits at bit 15.
            let take = ((8 - self.walker.step_bit) as usize).min(16 - acc_bits as usize);
            let sb = self.walker.step_bit;
            for (a, s) in acc.iter_mut().zip(self.walker.step.iter()).take(ch) {
                for i in 0..take {
                    let b = s[(sb + i as u8) as usize];
                    *a = (*a << 1) | u16::from(b);
                }
            }
            self.walker.step_bit += take as u8;
            self.walker.bits_read += take as u64;
            acc_bits += take as u8;
            if acc_bits == 16 {
                out[words_written..words_written + ch].copy_from_slice(&acc[..ch]);
                words_written += ch;
                acc = [0u16; 6];
                acc_bits = 0;
            }
        }
        Ok(words_written)
    }

    /// Seek to DSD bit position `bit` (per channel): the next
    /// [`read_words`](Self::read_words) starts there. Bit-exact — there is
    /// no filter state to warm, unlike the PCM decimator path.
    pub fn seek_bit(&mut self, bit: u64) -> Result<(), MusicError> {
        if bit < self.walker.bits_read {
            self.walker.rewind()?;
        }
        self.walker.skip_bits(bit - self.walker.bits_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ------------------------------------------------------------------
    // Fixture builders: minimal valid DSF / DFF files with known bit patterns
    // ------------------------------------------------------------------

    fn w32le(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn w64le(v: &mut Vec<u8>, x: u64) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn w32be(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn w64be(v: &mut Vec<u8>, x: u64) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    /// Build a stereo DSF: `blocks` blocks of `block_len` bytes per channel.
    /// `fill_ch0`/`fill_ch1` are the byte patterns (DC tests use 0xFF/0x00).
    fn build_dsf(blocks: usize, block_len: usize, fill_ch0: u8, fill_ch1: u8) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"DSD ");
        w64le(&mut v, 28);
        let file_size_pos = v.len();
        w64le(&mut v, 0); // patched later
        let data_ptr_pos = v.len();
        w64le(&mut v, 0); // patched later
                          // fmt chunk: 52 bytes total per the Sony spec
        v.extend_from_slice(b"fmt ");
        w64le(&mut v, 52);
        w32le(&mut v, 1); // version
        w32le(&mut v, 0); // format id: uncompressed
        w32le(&mut v, 2); // channel type: stereo
        w32le(&mut v, 2); // channels
        w32le(&mut v, 2_822_400);
        w32le(&mut v, 1); // bits per sample
        let sample_count = blocks as u64 * block_len as u64 * 8;
        w64le(&mut v, sample_count);
        w32le(&mut v, block_len as u32);
        w32le(&mut v, 0); // reserved
                          // data chunk
        let data_off = v.len();
        v.extend_from_slice(b"data");
        let payload_len = blocks * 2 * block_len;
        w64le(&mut v, 12 + payload_len as u64);
        for _ in 0..blocks {
            v.extend(std::iter::repeat_n(fill_ch0, block_len));
            v.extend(std::iter::repeat_n(fill_ch1, block_len));
        }
        let total = v.len() as u64;
        v[file_size_pos..file_size_pos + 8].copy_from_slice(&total.to_le_bytes());
        v[data_ptr_pos..data_ptr_pos + 8].copy_from_slice(&(data_off as u64).to_le_bytes());
        v
    }

    /// Build a stereo DFF with `frames` frame-interleaved frames.
    fn build_dff(frames: usize, fill_ch0: u8, fill_ch1: u8) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"FRM8");
        let form_size_pos = v.len();
        w64be(&mut v, 0); // patched later
        v.extend_from_slice(b"DSD ");
        // FVER
        v.extend_from_slice(b"FVER");
        w64be(&mut v, 4);
        w32be(&mut v, 0x0105_0000);
        // PROP > SND > (FS, CHNL, CMPR)
        let mut prop = Vec::new();
        prop.extend_from_slice(b"SND ");
        prop.extend_from_slice(b"FS  ");
        w64be(&mut prop, 4);
        w32be(&mut prop, 2_822_400);
        prop.extend_from_slice(b"CHNL");
        w64be(&mut prop, 2 + 8);
        prop.extend_from_slice(&2u16.to_be_bytes());
        prop.extend_from_slice(&[0u8; 8]); // channel IDs (ignored by parser)
        prop.extend_from_slice(b"CMPR");
        w64be(&mut prop, 4);
        prop.extend_from_slice(b"DSD ");
        v.extend_from_slice(b"PROP");
        w64be(&mut v, prop.len() as u64);
        v.extend_from_slice(&prop);
        // DSD chunk
        v.extend_from_slice(b"DSD ");
        w64be(&mut v, (frames * 2) as u64);
        for _ in 0..frames {
            v.push(fill_ch0);
            v.push(fill_ch1);
        }
        let total = v.len() as u64;
        v[form_size_pos..form_size_pos + 8].copy_from_slice(&(total - 12).to_be_bytes());
        v
    }

    fn mean_tail(samples: &[f32], skip: usize) -> f32 {
        let tail = &samples[skip..];
        tail.iter().sum::<f32>() / tail.len() as f32
    }

    // ------------------------------------------------------------------
    // Parser tests
    // ------------------------------------------------------------------

    #[test]
    fn bit_order_follows_container_spec() {
        // Sony DSF spec v1.01 §II, Annotation 4: with bits_per_sample = 1
        // the data is stored LSB-first. The spec's own example: dsd stream
        // value 0x01 (00000001) is stored as byte 0x80 — i.e. reading
        // LSB-first recovers the stream.
        assert_eq!(
            dsd_byte_time_order(0x80, true),
            [false, false, false, false, false, false, false, true]
        );
        assert_eq!(
            dsd_byte_time_order(0x01, true),
            [true, false, false, false, false, false, false, false]
        );
        // DFF (DSDIFF 1.5 §3.4.3) is MSB-first: the stream starts with the
        // most significant bit of the first byte.
        assert_eq!(
            dsd_byte_time_order(0x80, false),
            [true, false, false, false, false, false, false, false]
        );
        assert_eq!(
            dsd_byte_time_order(0x01, false),
            [false, false, false, false, false, false, false, true]
        );
    }

    #[test]
    fn dsf_parses_valid_header() {
        let bytes = build_dsf(2, 16, 0xFF, 0x00);
        let info = parse_dsd(&mut Cursor::new(&bytes)).unwrap();
        assert!(info.is_dsf);
        assert_eq!(info.channels, 2);
        assert_eq!(info.dsd_rate, 2_822_400);
        assert_eq!(info.samples_per_channel, 2 * 16 * 8);
        assert_eq!(info.layout, PayloadLayout::Blocked);
        assert_eq!(info.pcm_rate(), 88_200);
        assert_eq!(info.ratio(), 32);
    }

    #[test]
    fn dff_parses_valid_header() {
        let bytes = build_dff(64, 0xFF, 0x00);
        let info = parse_dsd(&mut Cursor::new(&bytes)).unwrap();
        assert!(!info.is_dsf);
        assert_eq!(info.channels, 2);
        assert_eq!(info.dsd_rate, 2_822_400);
        assert_eq!(info.samples_per_channel, 64 * 8);
        assert_eq!(info.layout, PayloadLayout::FrameInterleaved);
    }

    #[test]
    fn rejects_non_dsd_magic() {
        let err = parse_dsd(&mut Cursor::new(b"RIFF....junk")).unwrap_err();
        assert!(matches!(err, MusicError::BadRequest(_)), "{err:?}");
    }

    #[test]
    fn rejects_truncated_header() {
        let bytes = build_dsf(1, 16, 0xFF, 0x00);
        let err = parse_dsd(&mut Cursor::new(&bytes[..20])).unwrap_err();
        assert!(matches!(err, MusicError::Io(_)), "{err:?}");
    }

    #[test]
    fn rejects_bad_fmt_version() {
        let mut bytes = build_dsf(1, 16, 0xFF, 0x00);
        // fmt version lives at offset 28+12+0 = 40.
        bytes[40] = 2;
        let err = parse_dsd(&mut Cursor::new(&bytes)).unwrap_err();
        assert!(matches!(err, MusicError::BadRequest(_)), "{err:?}");
    }

    #[test]
    fn rejects_dst_compression() {
        let mut bytes = build_dff(8, 0xFF, 0x00);
        // Patch CMPR payload "DSD " -> "DST ".
        let pos = bytes
            .windows(4)
            .position(|w| w == b"CMPR")
            .expect("CMPR chunk");
        bytes[pos + 12..pos + 16].copy_from_slice(b"DST ");
        let err = parse_dsd(&mut Cursor::new(&bytes)).unwrap_err();
        assert!(matches!(err, MusicError::UnsupportedFormat(_)), "{err:?}");
    }

    #[test]
    fn pcm_rate_mapping() {
        assert_eq!(pcm_rate_for_dsd(2_822_400), Some(88_200));
        assert_eq!(pcm_rate_for_dsd(5_644_800), Some(176_400));
        assert_eq!(pcm_rate_for_dsd(11_289_600), Some(176_400));
        assert_eq!(pcm_rate_for_dsd(44_100), None);
    }

    // ------------------------------------------------------------------
    // Decimator tests
    // ------------------------------------------------------------------

    #[test]
    fn decimator_dc_in_gives_dc_out_at_expected_rate() {
        // Stereo: ch0 all-ones (+1.0 DC), ch1 all-zeros (−1.0 DC).
        let bytes = build_dsf(8, 64, 0xFF, 0x00);
        let mut rdr = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        assert_eq!(rdr.pcm_rate(), 88_200);
        assert_eq!(rdr.channels(), 2);

        // 8 blocks × 64 bytes × 8 bits / 32 = 128 PCM samples per channel.
        let mut out = vec![0.0f32; 128 * 2];
        let n = rdr.read_pcm(&mut out).unwrap();
        assert_eq!(n, 256);

        let ch0: Vec<f32> = out.iter().step_by(2).copied().collect();
        let ch1: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        // Skip the filter warmup, then DC must be exact-ish (DC gain = 1.0).
        assert!((mean_tail(&ch0, 64) - 1.0).abs() < 0.01, "ch0 dc");
        assert!((mean_tail(&ch1, 64) + 1.0).abs() < 0.01, "ch1 dc");
    }

    #[test]
    fn decimator_kills_undithered_1mhz_tone() {
        // 0xAA = 10101010: a tone at DSD_rate/2 = 1.4112 MHz, far into the
        // stopband. Output must be ~silence.
        let bytes = build_dsf(8, 64, 0xAA, 0xAA);
        let mut rdr = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        let mut out = vec![0.0f32; 128 * 2];
        let n = rdr.read_pcm(&mut out).unwrap();
        assert_eq!(n, 256);
        let peak: f32 = out.iter().skip(128).fold(0.0, |a, v| a.max(v.abs()));
        assert!(peak < 0.05, "stopband leak peak={peak}");
    }

    #[test]
    fn decimator_partial_density_gives_proportional_dc() {
        // 0xFE = 11111110 → 7/8 ones → +0.75 DC; 0x01 = 00000001 → −0.75.
        let bytes = build_dsf(8, 64, 0xFE, 0x01);
        let mut rdr = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        let mut out = vec![0.0f32; 128 * 2];
        rdr.read_pcm(&mut out).unwrap();
        let ch0: Vec<f32> = out.iter().step_by(2).copied().collect();
        let ch1: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!(
            (mean_tail(&ch0, 64) - 0.75).abs() < 0.05,
            "ch0 = {}",
            mean_tail(&ch0, 64)
        );
        assert!(
            (mean_tail(&ch1, 64) + 0.75).abs() < 0.05,
            "ch1 = {}",
            mean_tail(&ch1, 64)
        );
    }

    #[test]
    fn dff_frame_interleave_decodes_to_same_dc() {
        let bytes = build_dff(512, 0xFF, 0x00);
        let mut rdr = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        // 512 frames × 8 bits / 32 = 128 PCM samples per channel.
        let mut out = vec![0.0f32; 128 * 2];
        let n = rdr.read_pcm(&mut out).unwrap();
        assert_eq!(n, 256);
        let ch0: Vec<f32> = out.iter().step_by(2).copied().collect();
        let ch1: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!((mean_tail(&ch0, 64) - 1.0).abs() < 0.01);
        assert!((mean_tail(&ch1, 64) + 1.0).abs() < 0.01);
    }

    #[test]
    fn skip_pcm_lands_past_the_skip_point() {
        // First half of blocks: 0xFF (+1); second half: 0x00 (−1).
        let mut bytes = build_dsf(8, 64, 0xFF, 0xFF);
        // Patch the second 4 blocks' worth of payload to 0x00.
        // Payload starts after "data"+size = find it:
        let data_pos = bytes.windows(4).position(|w| w == b"data").unwrap();
        let payload = data_pos + 12;
        let half = 4 * 2 * 64;
        for b in bytes[payload + half..payload + 2 * half].iter_mut() {
            *b = 0x00;
        }
        let mut rdr = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        // Skip exactly half the PCM: 64 samples per channel.
        rdr.skip_pcm(64).unwrap();
        let mut out = vec![0.0f32; 64 * 2];
        let n = rdr.read_pcm(&mut out).unwrap();
        assert_eq!(n, 128);
        let ch0: Vec<f32> = out.iter().step_by(2).copied().collect();
        // Seek lands in the 0x00 half: expect −1 DC (the FIR is warmed, so
        // there is no post-seek transient to skip).
        assert!(
            (mean_tail(&ch0, 32) + 1.0).abs() < 0.05,
            "got {}",
            mean_tail(&ch0, 32)
        );
    }

    #[test]
    fn dsd_seek_is_sample_exact() {
        // Step pattern: first half of the payload 0xFF (+1), second half
        // 0x00 (−1). Seek to just past the step so the FIR history at the
        // seek point straddles it — without warm-up the first frames would
        // differ from a full decode.
        let blocks = 32;
        let block_len = 4096;
        let mut bytes = build_dsf(blocks, block_len, 0xFF, 0xFF);
        let data_pos = bytes.windows(4).position(|w| w == b"data").unwrap();
        let payload = data_pos + 12;
        let total_payload = blocks * 2 * block_len;
        for b in bytes[payload + total_payload / 2..payload + total_payload].iter_mut() {
            *b = 0x00;
        }
        let total_frames = (blocks * block_len * 8 / 32) as u64; // ratio 32
        let seek_frame = total_frames / 2 + 5;

        // Full decode for reference.
        let mut full = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        let mut full_pcm = vec![0.0f32; total_frames as usize * 2];
        let n = full.read_pcm(&mut full_pcm).unwrap();
        assert_eq!(n, full_pcm.len());

        // Seeked decode.
        let mut seeked = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        seeked.skip_pcm(seek_frame).unwrap();
        let mut out = vec![0.0f32; 2048 * 2];
        let m = seeked.read_pcm(&mut out).unwrap();
        assert_eq!(m, out.len());

        let base = seek_frame as usize * 2;
        let max_diff = out
            .iter()
            .zip(&full_pcm[base..base + out.len()])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(max_diff, 0.0, "DSD seek must be sample-exact");
    }

    #[test]
    fn dsd_seek_near_start_is_sample_exact() {
        // Seek point inside the warm-up window (fewer than taps bits into
        // the stream): warm-up consumes the whole prefix, reproducing the
        // full-decode state exactly.
        let mut bytes = build_dsf(4, 4096, 0xFF, 0xFF);
        let data_pos = bytes.windows(4).position(|w| w == b"data").unwrap();
        let payload = data_pos + 12;
        let total_payload = 4 * 2 * 4096;
        for b in bytes[payload + total_payload / 2..payload + total_payload].iter_mut() {
            *b = 0x00;
        }
        let total_frames = (4 * 4096 * 8 / 32) as u64;
        let seek_frame = 5u64; // 160 bits < 608-bit warm-up window

        let mut full = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        let mut full_pcm = vec![0.0f32; total_frames as usize * 2];
        let n = full.read_pcm(&mut full_pcm).unwrap();
        assert_eq!(n, full_pcm.len());

        let mut seeked = DsdPcmReader::open(Cursor::new(&bytes)).unwrap();
        seeked.skip_pcm(seek_frame).unwrap();
        let mut out = vec![0.0f32; 512 * 2];
        let m = seeked.read_pcm(&mut out).unwrap();
        assert_eq!(m, out.len());

        let base = seek_frame as usize * 2;
        let max_diff = out
            .iter()
            .zip(&full_pcm[base..base + out.len()])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(max_diff, 0.0, "near-start DSD seek must be sample-exact");
    }

    #[test]
    fn filter_design_is_sane() {
        let dec = Decimator::new(2_822_400, 2).unwrap();
        // Kaiser estimate for 75 dB, ~21.9 kHz transition at 2.8 MHz:
        // expect a few hundred taps, odd, and the description names the design.
        assert!(dec.taps() % 2 == 1);
        assert!((200..=1200).contains(&dec.taps()), "taps={}", dec.taps());
        let d = dec.describe();
        assert!(d.contains("DSD64"), "{d}");
        assert!(d.contains("88"), "{d}");
    }
}
