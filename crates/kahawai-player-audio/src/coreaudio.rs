//! Exclusive hog-mode DoP output via raw CoreAudio (macOS only).
//!
//! Bit-perfect contract: the engine hands this sink raw DoP frames and the
//! sink must deliver them to the DAC untouched — 24-bit physical stream
//! format, exact sample rate, no mixer, no DSP. Achieved with:
//!
//! 1. **Hog mode** (`kAudioDevicePropertyHogMode`): exclusive access, so
//!    the system mixer can't touch the stream.
//! 2. **Nominal sample rate** set to the exact DoP rate (176.4 / 352.8 /
//!    705.6 kHz) and *verified by readback* — a device that silently
//!    stays at 44.1 kHz would play garbage.
//! 3. **Stream physical format** forced to signed-integer PCM and verified
//!    by readback — CoreAudio HAL streams default to Float32, which would
//!    destroy the DoP marker encoding. Packed 24-bit is preferred; DACs
//!    that carry their 24-bit slot as 32 bits (XMOS XU316 firmware such as
//!    the FiiO K15 offers only 16 and 32) get a left-aligned 32-bit
//!    container instead: the same 24 DoP bits in the top of each word.
//!
//! If any step fails, `open()` returns `Err` and the device is left as it
//! was (hog released, rate restored): the engine's capability query
//! (`dop_output_rate`, backed by the device's available-rate list) routes
//! unsupported devices to the FLAC fallback *before* we get here.
//!
//! Underrun policy: the IO proc never blocks or allocates. If the engine
//! doesn't keep the ring fed, the proc emits a valid DoP *silence* frame
//! (marker bytes keep alternating, DSD payload = 0x69, the DSD silence
//! level) so the DAC's marker tracking never desyncs, and the underrun is
//! counted.
//!
//! This file is `cfg(target_os = "macos")`: it cannot be compiled or run
//! on the Linux CI. First real validation happens on the Mac (C3).

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use coreaudio_sys::*;
use kahawai_core::{MusicError, Track};
use kahawai_player_core::{dop_pcm_rate, AudioSink, OutputPath, PcmChunk, SinkState};
use ringbuf::{traits::*, HeapCons, HeapProd, HeapRb};

/// DoP rates we can emit, ascending.
const DOP_RATES: [u32; 3] = [176_400, 352_800, 705_600];
/// DSD silence byte: mid-level sigma-delta idle pattern.
const DSD_SILENCE: u8 = 0x69;
/// Marker bytes alternate 0x05 / 0xFA per frame (server dop.rs).
const MARKER_EVEN: u8 = 0x05;
const MARKER_ODD: u8 = 0xFA;
/// How long `write_dop` waits for the IO proc to drain before giving up.
const WRITE_DEADLINE: Duration = Duration::from_secs(2);
/// Ring capacity ceiling (2 s at the fastest rate, stereo).
const RING_CAP_MAX: usize = 16 * 1024 * 1024;

const NO_ERR: OSStatus = 0;

fn os(status: OSStatus, what: &str) -> Result<(), MusicError> {
    if status == NO_ERR {
        Ok(())
    } else {
        Err(MusicError::Audio(format!(
            "CoreAudio {what} failed: OSStatus {status}"
        )))
    }
}

fn prop_addr(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn default_output_device() -> Result<AudioObjectID, MusicError> {
    let addr = prop_addr(kAudioHardwarePropertyDefaultOutputDevice);
    let mut device: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    os(
        unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject,
                &addr,
                0,
                ptr::null(),
                &mut size,
                &mut device as *mut _ as *mut c_void,
            )
        },
        "get default output device",
    )?;
    if device == 0 {
        return Err(MusicError::Audio("no default output device".into()));
    }
    Ok(device)
}

/// Discrete nominal sample rates the device reports.
fn available_sample_rates(device: AudioObjectID) -> Result<Vec<f64>, MusicError> {
    let addr = prop_addr(kAudioDevicePropertyAvailableNominalSampleRates);
    let mut size: u32 = 0;
    os(
        unsafe { AudioObjectGetPropertyDataSize(device, &addr, 0, ptr::null(), &mut size) },
        "get available-rate size",
    )?;
    let count = size as usize / std::mem::size_of::<AudioValueRange>();
    let mut ranges: Vec<AudioValueRange> = (0..count)
        .map(|_| AudioValueRange {
            mMinimum: 0.0,
            mMaximum: 0.0,
        })
        .collect();
    let mut size2 = size;
    os(
        unsafe {
            AudioObjectGetPropertyData(
                device,
                &addr,
                0,
                ptr::null(),
                &mut size2,
                ranges.as_mut_ptr() as *mut c_void,
            )
        },
        "get available rates",
    )?;
    let mut out = Vec::new();
    for r in ranges {
        // Devices report discrete rates as degenerate ranges; keep both
        // endpoints of any true range so we never miss a rate.
        out.push(r.mMinimum);
        if (r.mMaximum - r.mMinimum).abs() > f64::EPSILON {
            out.push(r.mMaximum);
        }
    }
    Ok(out)
}

fn get_nominal_rate(device: AudioObjectID) -> Result<f64, MusicError> {
    let addr = prop_addr(kAudioDevicePropertyNominalSampleRate);
    let mut rate: f64 = 0.0;
    let mut size = std::mem::size_of::<f64>() as u32;
    os(
        unsafe {
            AudioObjectGetPropertyData(
                device,
                &addr,
                0,
                ptr::null(),
                &mut size,
                &mut rate as *mut _ as *mut c_void,
            )
        },
        "get nominal sample rate",
    )?;
    Ok(rate)
}

fn set_nominal_rate(device: AudioObjectID, rate: f64) -> Result<(), MusicError> {
    let addr = prop_addr(kAudioDevicePropertyNominalSampleRate);
    os(
        unsafe {
            AudioObjectSetPropertyData(
                device,
                &addr,
                0,
                ptr::null(),
                std::mem::size_of::<f64>() as u32,
                &rate as *const _ as *const c_void,
            )
        },
        "set nominal sample rate",
    )
}

fn get_hog_pid(device: AudioObjectID) -> Result<i32, MusicError> {
    let addr = prop_addr(kAudioDevicePropertyHogMode);
    let mut pid: i32 = 0;
    let mut size = std::mem::size_of::<i32>() as u32;
    os(
        unsafe {
            AudioObjectGetPropertyData(
                device,
                &addr,
                0,
                ptr::null(),
                &mut size,
                &mut pid as *mut _ as *mut c_void,
            )
        },
        "get hog mode",
    )?;
    Ok(pid)
}

fn set_hog_pid(device: AudioObjectID, pid: i32) -> Result<(), MusicError> {
    let addr = prop_addr(kAudioDevicePropertyHogMode);
    os(
        unsafe {
            AudioObjectSetPropertyData(
                device,
                &addr,
                0,
                ptr::null(),
                std::mem::size_of::<i32>() as u32,
                &pid as *const _ as *const c_void,
            )
        },
        "set hog mode",
    )
}

fn output_streams(device: AudioObjectID) -> Result<Vec<AudioStreamID>, MusicError> {
    let addr = prop_addr(kAudioDevicePropertyStreams);
    let mut size: u32 = 0;
    os(
        unsafe { AudioObjectGetPropertyDataSize(device, &addr, 0, ptr::null(), &mut size) },
        "get stream list size",
    )?;
    let count = size as usize / std::mem::size_of::<AudioStreamID>();
    let mut ids: Vec<AudioStreamID> = (0..count).map(|_| 0).collect();
    let mut size2 = size;
    os(
        unsafe {
            AudioObjectGetPropertyData(
                device,
                &addr,
                0,
                ptr::null(),
                &mut size2,
                ids.as_mut_ptr() as *mut c_void,
            )
        },
        "get stream list",
    )?;
    Ok(ids)
}

/// How one 24-bit DoP/PCM sample is carried on the device's stream. The
/// engine and the ring always hold packed 24-bit; the render proc expands
/// to the device's container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    /// 3 bytes per sample, as the server packs them.
    Packed24,
    /// 4 bytes per sample, 24-bit value left-aligned (low byte zero):
    /// how UAC devices with a 4-byte subslot appear in CoreAudio.
    Wide32,
}

impl Container {
    const PREFERENCE: [Container; 2] = [Container::Packed24, Container::Wide32];

    fn bytes_per_sample(self) -> usize {
        match self {
            Container::Packed24 => 3,
            Container::Wide32 => 4,
        }
    }

    fn bits(self) -> u32 {
        self.bytes_per_sample() as u32 * 8
    }
}

/// How the IO callback's buffers are laid out: the stream's VIRTUAL format,
/// which can differ from the physical format we set for the hardware. On the
/// FiiO K15 (XMOS XU316) it is fixed at Float32 and the driver converts to the
/// 32-bit integer the DAC takes; writing integers there would be read as floats
/// (loud noise). A 24-bit sample scaled by exactly 2^-23 is exact in Float32 and
/// converts back to the identical integer, so this stays bit-perfect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IoLayout {
    /// 3 bytes per sample, as the ring holds them.
    Packed24,
    /// 4 bytes per sample, 24-bit value left-aligned in a signed 32-bit integer.
    Int32,
    /// 4 bytes per sample, Float32 in [-1, 1): the 24-bit value divided by 2^23.
    Float32,
}

/// Decide the IO layout from the stream's virtual format read back after the
/// physical format was set. `None` when it is neither our integer container
/// nor interleaved Float32 at the right rate and channel count.
fn classify_virtual(
    virt: &AudioStreamBasicDescription,
    rate: u32,
    channels: u16,
    container: Container,
) -> Option<IoLayout> {
    if format_matches(virt, rate, channels, container) {
        return Some(match container {
            Container::Packed24 => IoLayout::Packed24,
            Container::Wide32 => IoLayout::Int32,
        });
    }
    let float32 = virt.mFormatFlags & kAudioFormatFlagIsFloat != 0
        && virt.mBitsPerChannel == 32
        && virt.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0
        && virt.mChannelsPerFrame == channels as u32
        && virt.mBytesPerFrame == channels as u32 * 4
        && (virt.mSampleRate - rate as f64).abs() < 1.0;
    float32.then_some(IoLayout::Float32)
}

fn dop_stream_format(
    rate: u32,
    channels: u16,
    container: Container,
) -> AudioStreamBasicDescription {
    let bps = container.bytes_per_sample() as u32;
    AudioStreamBasicDescription {
        mSampleRate: rate as f64,
        mFormatID: kAudioFormatLinearPCM,
        // Packed signed-integer, native (little) endian: the DoP bytes
        // land on the wire exactly as the server packed them.
        mFormatFlags: kAudioFormatFlagIsPacked | kAudioFormatFlagIsSignedInteger,
        mBytesPerPacket: channels as u32 * bps,
        mFramesPerPacket: 1,
        mBytesPerFrame: channels as u32 * bps,
        mChannelsPerFrame: channels as u32,
        mBitsPerChannel: container.bits(),
        mReserved: 0,
    }
}

/// Integer physical formats a stream offers at `rate`, as containers we
/// can drive. Capability query only.
fn offered_containers(device: AudioObjectID, rate: u32, channels: u16) -> Vec<Container> {
    let Ok(streams) = output_streams(device) else {
        return Vec::new();
    };
    let Some(stream) = streams.first().copied() else {
        return Vec::new();
    };
    let addr = prop_addr(kAudioStreamPropertyAvailablePhysicalFormats);
    let mut size: u32 = 0;
    if unsafe { AudioObjectGetPropertyDataSize(stream, &addr, 0, ptr::null(), &mut size) } != NO_ERR
    {
        return Vec::new();
    }
    let n = size as usize / std::mem::size_of::<AudioStreamRangedDescription>();
    let mut fmts: Vec<AudioStreamRangedDescription> = Vec::with_capacity(n);
    let mut size2 = size;
    let status = unsafe {
        AudioObjectGetPropertyData(
            stream,
            &addr,
            0,
            ptr::null(),
            &mut size2,
            fmts.as_mut_ptr() as *mut c_void,
        )
    };
    if status != NO_ERR {
        return Vec::new();
    }
    unsafe { fmts.set_len((size2 as usize / std::mem::size_of::<AudioStreamRangedDescription>()).min(n)) };
    Container::PREFERENCE
        .into_iter()
        .filter(|c| {
            fmts.iter().any(|f| {
                let a = &f.mFormat;
                a.mFormatID == kAudioFormatLinearPCM
                    && a.mFormatFlags & kAudioFormatFlagIsSignedInteger != 0
                    && a.mFormatFlags & kAudioFormatFlagIsFloat == 0
                    && a.mBitsPerChannel == c.bits()
                    && a.mChannelsPerFrame == channels as u32
                    && (a.mSampleRate - rate as f64).abs() < 1.0
            })
        })
        .collect()
}

fn set_format_prop(
    stream: AudioStreamID,
    selector: u32,
    asbd: &AudioStreamBasicDescription,
    what: &str,
) -> Result<(), MusicError> {
    let addr = prop_addr(selector);
    os(
        unsafe {
            AudioObjectSetPropertyData(
                stream,
                &addr,
                0,
                ptr::null(),
                std::mem::size_of::<AudioStreamBasicDescription>() as u32,
                asbd as *const _ as *const c_void,
            )
        },
        what,
    )
}

fn get_format_prop(
    stream: AudioStreamID,
    selector: u32,
    what: &str,
) -> Result<AudioStreamBasicDescription, MusicError> {
    let addr = prop_addr(selector);
    let mut asbd: AudioStreamBasicDescription = AudioStreamBasicDescription {
        mSampleRate: 0.0,
        mFormatID: 0,
        mFormatFlags: 0,
        mBytesPerPacket: 0,
        mFramesPerPacket: 0,
        mBytesPerFrame: 0,
        mChannelsPerFrame: 0,
        mBitsPerChannel: 0,
        mReserved: 0,
    };
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    os(
        unsafe {
            AudioObjectGetPropertyData(
                stream,
                &addr,
                0,
                ptr::null(),
                &mut size,
                &mut asbd as *mut _ as *mut c_void,
            )
        },
        what,
    )?;
    Ok(asbd)
}

/// What the hardware is sent.
fn set_stream_format(stream: AudioStreamID, asbd: &AudioStreamBasicDescription) -> Result<(), MusicError> {
    set_format_prop(stream, kAudioStreamPropertyPhysicalFormat, asbd, "set stream physical format")
}

fn get_stream_format(stream: AudioStreamID) -> Result<AudioStreamBasicDescription, MusicError> {
    get_format_prop(stream, kAudioStreamPropertyPhysicalFormat, "get stream physical format")
}

/// What the IO callback's buffers actually contain. Independent of the
/// physical format: if it is left at Float32 while we write integers, the
/// device reads our samples as floats, which is loud garbage. It does not
/// always follow the physical format (it only moves when the physical format
/// *changes*), so it must be set and verified explicitly.
fn set_virtual_format(stream: AudioStreamID, asbd: &AudioStreamBasicDescription) -> Result<(), MusicError> {
    set_format_prop(stream, kAudioStreamPropertyVirtualFormat, asbd, "set stream virtual format")
}

fn get_virtual_format(stream: AudioStreamID) -> Result<AudioStreamBasicDescription, MusicError> {
    get_format_prop(stream, kAudioStreamPropertyVirtualFormat, "get stream virtual format")
}

/// Does this stream format match what we asked for (an integer container of
/// the right width, at the right rate and channel count)?
fn format_matches(back: &AudioStreamBasicDescription, rate: u32, channels: u16, container: Container) -> bool {
    (back.mSampleRate - rate as f64).abs() < 1.0
        && back.mBitsPerChannel == container.bits()
        && back.mChannelsPerFrame == channels as u32
        && back.mBytesPerFrame == channels as u32 * container.bytes_per_sample() as u32
        && back.mFormatFlags & kAudioFormatFlagIsFloat == 0
        && back.mFormatFlags & kAudioFormatFlagIsSignedInteger != 0
}

/// State owned by the IO proc: the render thread only touches the
/// consumer half of the ring, the engine thread only the producer half.
struct RenderState {
    cons: HeapCons<u8>,
    underruns: Arc<AtomicU64>,
    channels: u16,
    /// How the IO buffers are laid out (the ring is always packed 24).
    layout: IoLayout,
    /// Set by the engine thread to drop whatever is still queued (a seek or
    /// skip on a session that stays open); the render thread clears it once
    /// the ring is emptied, which is the engine's cue that it is safe to write.
    flush: Arc<AtomicBool>,
    /// Marker for the next underrun-silence frame (DoP only).
    marker: u8,
    /// What an underrun sounds like: DoP needs valid marker frames, plain
    /// PCM needs true zeros (DoP silence bytes would be a loud burst).
    silence: Silence,
}

/// The kind of stream the exclusive device carries, i.e. what "silence" is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Silence {
    /// DSD-over-PCM: DSD silence payload plus alternating marker bytes.
    Dop,
    /// Ordinary PCM: all-zero samples.
    Pcm,
}

/// Fill one underrun frame (`channels` packed 24-bit samples) and advance
/// the DoP marker when that is the stream type. Real-time safe.
fn fill_underrun_frame(dst: &mut [u8], channels: u16, silence: Silence, marker: &mut u8) {
    match silence {
        Silence::Pcm => dst.fill(0),
        Silence::Dop => {
            for ch in 0..channels as usize {
                dst[ch * 3] = DSD_SILENCE;
                dst[ch * 3 + 1] = DSD_SILENCE;
                dst[ch * 3 + 2] = *marker;
            }
            *marker = if *marker == MARKER_EVEN {
                MARKER_ODD
            } else {
                MARKER_EVEN
            };
        }
    }
}

unsafe extern "C" fn dop_io_proc(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    _input: *const AudioBufferList,
    _in_time: *const AudioTimeStamp,
    output: *mut AudioBufferList,
    _out_time: *const AudioTimeStamp,
    client_data: *mut c_void,
) -> OSStatus {
    let state = &mut *(client_data as *mut RenderState);
    let out = &mut *output;
    let n_bufs = out.mNumberBuffers;
    for i in 0..n_bufs {
        // AudioBufferList is a flexible-array struct; buffers follow the
        // header contiguously (the universal CoreAudio layout).
        let buf = &mut *out.mBuffers.as_mut_ptr().add(i as usize);
        let bytes =
            std::slice::from_raw_parts_mut(buf.mData as *mut u8, buf.mDataByteSize as usize);
        render_into(state, bytes);
    }
    NO_ERR
}

/// Fill `bytes` with whole DoP frames from the ring; underruns become
/// valid DoP silence frames so the marker stream never desyncs.
fn render_into(state: &mut RenderState, bytes: &mut [u8]) {
    let ring_frame = state.channels as usize * 3;
    if ring_frame == 0 {
        return;
    }
    if state.flush.load(Ordering::Acquire) {
        state.cons.clear();
        state.flush.store(false, Ordering::Release);
    }
    match state.layout {
        IoLayout::Packed24 => {
            let frames = bytes.len() / ring_frame;
            for f in 0..frames {
                let dst = &mut bytes[f * ring_frame..(f + 1) * ring_frame];
                pop_frame(state, dst);
            }
        }
        IoLayout::Int32 | IoLayout::Float32 => {
            let out_frame = state.channels as usize * 4;
            let float = state.layout == IoLayout::Float32;
            let mut scratch = [0u8; 8 * 3];
            let frames = bytes.len() / out_frame;
            for f in 0..frames {
                let src = &mut scratch[..ring_frame];
                pop_frame(state, src);
                let dst = &mut bytes[f * out_frame..(f + 1) * out_frame];
                for ch in 0..state.channels as usize {
                    let b = [src[ch * 3], src[ch * 3 + 1], src[ch * 3 + 2]];
                    if float {
                        // Sign-extend the 24-bit word, scale by 2^-23 (exact).
                        let v = i32::from_le_bytes([b[0], b[1], b[2], if b[2] & 0x80 != 0 { 0xFF } else { 0 }]);
                        dst[ch * 4..ch * 4 + 4].copy_from_slice(&(v as f32 / 8_388_608.0).to_le_bytes());
                    } else {
                        // 24-bit little-endian sample -> top three bytes of a
                        // 32-bit little-endian word.
                        dst[ch * 4] = 0;
                        dst[ch * 4 + 1] = b[0];
                        dst[ch * 4 + 2] = b[1];
                        dst[ch * 4 + 3] = b[2];
                    }
                }
            }
        }
    }
}

/// Give a DoP frame the next marker in the alternating 0x05 / 0xFA sequence.
/// The renderer owns the marker phase: audio frames from a new stream, and the
/// silence frames it emits between streams, must form one unbroken
/// alternation, or the DAC drops out of DSD mode and plays the frames as PCM
/// (loud noise). The server's markers are the same sequence, so on a
/// continuous stream this rewrites nothing.
fn stamp_marker(frame: &mut [u8], channels: u16, marker: &mut u8) {
    for ch in 0..channels as usize {
        frame[ch * 3 + 2] = *marker;
    }
    *marker = if *marker == MARKER_EVEN { MARKER_ODD } else { MARKER_EVEN };
}

/// One packed-24 frame from the ring, or the stream's own silence (counted
/// as an underrun) when the ring is short.
fn pop_frame(state: &mut RenderState, dst: &mut [u8]) {
    if state.cons.pop_slice(dst) == dst.len() {
        if state.silence == Silence::Dop {
            stamp_marker(dst, state.channels, &mut state.marker);
        }
        return;
    }
    // Underrun: DoP keeps its markers alternating so the DAC never loses
    // sync; PCM gets zeros.
    state.underruns.fetch_add(1, Ordering::Relaxed);
    fill_underrun_frame(dst, state.channels, state.silence, &mut state.marker);
}

/// Every audio device the HAL knows about.
fn all_devices() -> Result<Vec<AudioObjectID>, MusicError> {
    let addr = prop_addr(kAudioHardwarePropertyDevices);
    let mut size: u32 = 0;
    os(
        unsafe {
            AudioObjectGetPropertyDataSize(
                kAudioObjectSystemObject,
                &addr,
                0,
                ptr::null(),
                &mut size,
            )
        },
        "get device list size",
    )?;
    let mut ids = vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
    let mut size2 = size;
    os(
        unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject,
                &addr,
                0,
                ptr::null(),
                &mut size2,
                ids.as_mut_ptr() as *mut c_void,
            )
        },
        "get device list",
    )?;
    ids.truncate(size2 as usize / std::mem::size_of::<AudioObjectID>());
    Ok(ids)
}

/// Device name as cpal reports it (`kAudioDevicePropertyDeviceNameCFString`),
/// so the name the UI picked from cpal's list matches here.
fn device_name_of(device: AudioObjectID) -> Option<String> {
    let addr = prop_addr(kAudioDevicePropertyDeviceNameCFString);
    let mut cf: CFStringRef = ptr::null();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut cf as *mut CFStringRef as *mut c_void,
        )
    };
    if status != NO_ERR || cf.is_null() {
        return None;
    }
    let out = unsafe {
        let len = CFStringGetLength(cf);
        let cap = CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) + 1;
        let mut buf = vec![0u8; cap as usize];
        let ok = CFStringGetCString(cf, buf.as_mut_ptr() as *mut _, cap, kCFStringEncodingUTF8);
        (ok != 0).then(|| {
            std::ffi::CStr::from_ptr(buf.as_ptr() as *const _)
                .to_string_lossy()
                .into_owned()
        })
    };
    unsafe { CFRelease(cf as CFTypeRef) };
    out
}

/// First output-capable device with this name.
fn find_output_device(name: &str) -> Option<AudioObjectID> {
    all_devices().ok()?.into_iter().find(|&d| {
        device_name_of(d).as_deref() == Some(name)
            && output_streams(d).map(|s| !s.is_empty()).unwrap_or(false)
    })
}

/// The chosen device, or the system default when none is chosen or it is
/// not connected any more (same fallback as the PCM sink).
fn resolve_device(name: Option<&str>) -> Result<AudioObjectID, MusicError> {
    if let Some(n) = name {
        match find_output_device(n) {
            Some(d) => return Ok(d),
            None => tracing::warn!(device = %n, "output device not found; using system default"),
        }
    }
    default_output_device()
}

/// DoP rates the given output device (`None` = system default) can carry:
/// reported as available nominal rates *and* offering an integer stream
/// format we can drive at that rate.
pub fn supported_dop_rates(device_name: Option<&str>) -> Result<Vec<u32>, MusicError> {
    let device = resolve_device(device_name)?;
    let avail = available_sample_rates(device)?;
    Ok(DOP_RATES
        .into_iter()
        .filter(|r| avail.iter().any(|a| (*a - *r as f64).abs() < 1.0))
        .filter(|r| !offered_containers(device, *r, 2).is_empty())
        .collect())
}

/// How a device is connected, as a short stable name for the UI and for the
/// "is this an external DAC" rule.
fn transport_name(device: AudioObjectID) -> &'static str {
    let addr = prop_addr(kAudioDevicePropertyTransportType);
    let mut t: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(device, &addr, 0, ptr::null(), &mut size, &mut t as *mut _ as *mut c_void)
    };
    if status != NO_ERR {
        return "unknown";
    }
    match t {
        x if x == kAudioDeviceTransportTypeUSB => "usb",
        x if x == kAudioDeviceTransportTypeThunderbolt => "thunderbolt",
        x if x == kAudioDeviceTransportTypeFireWire => "firewire",
        x if x == kAudioDeviceTransportTypeBuiltIn => "built-in",
        x if x == kAudioDeviceTransportTypeBluetooth || x == kAudioDeviceTransportTypeBluetoothLE => "bluetooth",
        x if x == kAudioDeviceTransportTypeHDMI || x == kAudioDeviceTransportTypeDisplayPort => "hdmi",
        x if x == kAudioDeviceTransportTypeAirPlay => "airplay",
        x if x == kAudioDeviceTransportTypeVirtual || x == kAudioDeviceTransportTypeAggregate => "virtual",
        _ => "other",
    }
}

/// External DAC-class connections: worth taking exclusive control of.
fn is_external_transport(name: &str) -> bool {
    matches!(name, "usb" | "thunderbolt" | "firewire")
}

/// Everything the Settings screen shows about an output device: what it is,
/// how it is connected, and what it can carry.
pub fn device_capabilities(device_name: Option<&str>) -> Option<crate::DeviceCapabilities> {
    let device = resolve_device(device_name).ok()?;
    let mut rates: Vec<u32> = available_sample_rates(device)
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.round() as u32)
        .filter(|r| *r > 0)
        .collect();
    rates.sort_unstable();
    rates.dedup();
    // Integer bit depths the first output stream offers at any rate.
    let mut depths: Vec<u32> = Vec::new();
    let mut float32 = false;
    if let Some(stream) = output_streams(device).ok().and_then(|s| s.first().copied()) {
        for f in all_physical_formats(stream) {
            if f.mFormatFlags & kAudioFormatFlagIsFloat != 0 {
                float32 = true;
            } else if !depths.contains(&f.mBitsPerChannel) {
                depths.push(f.mBitsPerChannel);
            }
        }
    }
    depths.sort_unstable();
    let transport = transport_name(device);
    let dop_rates: Vec<u32> = DOP_RATES
        .into_iter()
        .filter(|r| rates.contains(r))
        .filter(|r| !offered_containers(device, *r, 2).is_empty())
        .collect();
    Some(crate::DeviceCapabilities {
        name: device_name_of(device).unwrap_or_default(),
        transport,
        external_dac: is_external_transport(transport),
        sample_rates: rates,
        bit_depths: depths,
        float32,
        dop_rates,
        exclusive_available: true,
    })
}

/// What the device is doing right now, read from CoreAudio (not from what
/// the player believes): its current rate, its current stream format, and
/// whether this process holds it exclusively. Cheap; safe to poll.
pub fn device_live_state(device_name: Option<&str>) -> Option<crate::DeviceLiveState> {
    let device = resolve_device(device_name).ok()?;
    let rate = get_nominal_rate(device).ok()?;
    let (bits, float) = output_streams(device)
        .ok()
        .and_then(|s| s.first().copied())
        .and_then(|s| get_stream_format(s).ok())
        .map(|f| (f.mBitsPerChannel, f.mFormatFlags & kAudioFormatFlagIsFloat != 0))
        .unwrap_or((0, false));
    let exclusive = get_hog_pid(device).map(|p| p == std::process::id() as i32).unwrap_or(false);
    Some(crate::DeviceLiveState {
        name: device_name_of(device).unwrap_or_default(),
        rate_hz: rate.round() as u32,
        bit_depth: bits,
        float,
        exclusive,
    })
}

/// Every physical format a stream offers (all rates).
fn all_physical_formats(stream: AudioStreamID) -> Vec<AudioStreamBasicDescription> {
    let addr = prop_addr(kAudioStreamPropertyAvailablePhysicalFormats);
    let mut size: u32 = 0;
    if unsafe { AudioObjectGetPropertyDataSize(stream, &addr, 0, ptr::null(), &mut size) } != NO_ERR {
        return Vec::new();
    }
    let n = size as usize / std::mem::size_of::<AudioStreamRangedDescription>();
    let mut fmts: Vec<AudioStreamRangedDescription> = Vec::with_capacity(n);
    let mut size2 = size;
    let status = unsafe {
        AudioObjectGetPropertyData(stream, &addr, 0, ptr::null(), &mut size2, fmts.as_mut_ptr() as *mut c_void)
    };
    if status != NO_ERR {
        return Vec::new();
    }
    unsafe { fmts.set_len((size2 as usize / std::mem::size_of::<AudioStreamRangedDescription>()).min(n)) };
    fmts.into_iter().map(|f| f.mFormat).collect()
}

/// Name of the device a sink would open for `device_name` (`None` = the
/// system default, resolved to its real name).
pub fn resolved_device_name(device_name: Option<&str>) -> Option<String> {
    let device = resolve_device(device_name).ok()?;
    device_name_of(device)
}

/// A stream's formats before we touched it.
type SavedFormat = (AudioStreamID, AudioStreamBasicDescription, AudioStreamBasicDescription);

pub struct CoreAudioDopSink {
    device: AudioObjectID,
    /// Chosen output device by name; `None` = the system default.
    device_name: Option<String>,
    io_proc: Option<AudioDeviceIOProcID>,
    /// Owned render state handed to the IO proc; null when no proc.
    /// Only dereferenced by the render thread (or freed here after the
    /// proc is destroyed) — never touched from the engine thread.
    render_state: *mut RenderState,
    producer: Option<HeapProd<u8>>,
    active_rate: u32,
    channels: u16,
    container: Container,
    /// How the IO buffers are laid out (may be Float32 even when the hardware format is integer).
    layout: IoLayout,
    /// What the open session carries (DoP vs plain PCM); a session is only
    /// reused for the same kind.
    silence: Silence,
    /// Shared with the render thread's `RenderState`; see there.
    flush: Arc<AtomicBool>,
    saved_rate: f64,
    /// Every output stream's physical format before we changed it.
    saved_formats: Vec<SavedFormat>,
    state: SinkState,
    underruns: Arc<AtomicU64>,
}

// The engine thread owns the sink; the render thread only touches the
// ring consumer inside RenderState (a lock-free SPSC pair). The raw
// pointer is never dereferenced on the engine thread while the proc
// lives: it is created before the proc and freed after the proc is
// destroyed.
unsafe impl Send for CoreAudioDopSink {}

impl CoreAudioDopSink {
    pub fn new() -> Self {
        Self {
            device: 0,
            device_name: None,
            io_proc: None,
            render_state: ptr::null_mut(),
            producer: None,
            active_rate: 0,
            channels: 0,
            container: Container::Packed24,
            layout: IoLayout::Packed24,
            silence: Silence::Dop,
            flush: Arc::new(AtomicBool::new(false)),
            saved_rate: 0.0,
            saved_formats: Vec::new(),
            state: SinkState::Stopped,
            underruns: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Best-effort unwind: stop IO, destroy the proc, free render state,
    /// restore the nominal rate, release hog mode.
    fn release(&mut self) {
        unsafe {
            if self.device != 0 {
                if let Some(proc) = self.io_proc {
                    let _ = AudioDeviceStop(self.device, proc);
                    let _ = AudioDeviceDestroyIOProcID(self.device, proc);
                }
                if !self.render_state.is_null() {
                    let _ = Box::from_raw(self.render_state);
                    self.render_state = ptr::null_mut();
                }
                restore_device(self.device, self.saved_rate, &self.saved_formats);
                // -1 = no owner: release hog mode.
                let _ = set_hog_pid(self.device, -1);
            }
        }
        self.io_proc = None;
        self.producer = None;
        self.device = 0;
        self.active_rate = 0;
        self.channels = 0;
        self.container = Container::Packed24;
        self.layout = IoLayout::Packed24;
        self.saved_rate = 0.0;
        self.saved_formats.clear();
        self.state = SinkState::Stopped;
    }

    /// Acquire hog mode, switch to `rate`, and force an integer stream
    /// format. Every step is verified by readback; any failure unwinds the
    /// device to how it was (stream formats, rate, hog).
    fn acquire(&mut self, rate: u32, channels: u16) -> Result<(), MusicError> {
        let device = resolve_device(self.device_name.as_deref())?;
        self.saved_rate = get_nominal_rate(device)?;

        // 1. Hog mode: exclusive access.
        let pid = std::process::id() as i32;
        set_hog_pid(device, pid).map_err(|e| {
            MusicError::Audio(format!(
                "hog mode refused (another app may hold the device): {e}"
            ))
        })?;
        if get_hog_pid(device)? != pid {
            let _ = set_hog_pid(device, -1);
            return Err(MusicError::Audio(
                "hog mode did not stick; refusing DoP".into(),
            ));
        }

        // Remember every stream's format before touching anything: setting a
        // stream's physical format pins the device at that rate, and the
        // nominal rate alone will not move it back.
        let streams = match output_streams(device) {
            Ok(s) if !s.is_empty() => s,
            other => {
                let _ = set_hog_pid(device, -1);
                return Err(match other {
                    Err(e) => e,
                    _ => MusicError::Audio("device has no output streams".into()),
                });
            }
        };
        self.saved_formats = streams
            .iter()
            .filter_map(|s| {
                let phys = get_stream_format(*s).ok()?;
                let virt = get_virtual_format(*s).unwrap_or(phys);
                Some((*s, phys, virt))
            })
            .collect();
        let unwind = |this: &mut Self| {
            restore_device(device, this.saved_rate, &this.saved_formats);
            let _ = set_hog_pid(device, -1);
            this.saved_formats.clear();
        };

        // 2. Exact sample rate, verified by readback.
        if !set_rate_and_wait(device, rate as f64) {
            unwind(self);
            return Err(MusicError::Audio(format!(
                "device would not switch to {rate} Hz; refusing DoP"
            )));
        }

        // 3. Integer physical stream format on every output stream, verified
        // by readback. (HAL streams default to Float32, which would destroy
        // the DoP encoding.) Packed 24-bit if the device takes it, else a
        // left-aligned 32-bit container.
        let mut chosen = None;
        'containers: for container in Container::PREFERENCE {
            let asbd = dop_stream_format(rate, channels, container);
            let mut layout = None;
            for s in &streams {
                // Physical: what goes to the hardware.
                if set_stream_format(*s, &asbd).is_err() {
                    continue 'containers;
                }
                match get_stream_format(*s) {
                    Ok(back) if format_matches(&back, rate, channels, container) => {}
                    _ => continue 'containers,
                }
                // Virtual: what our IO callback's buffers hold. Ask for the same
                // integer format (some devices honour it), then read back what
                // it really is, and render in that layout.
                let _ = set_virtual_format(*s, &asbd);
                let this = get_virtual_format(*s)
                    .ok()
                    .and_then(|v| classify_virtual(&v, rate, channels, container));
                match (layout, this) {
                    (_, None) => continue 'containers,
                    (None, Some(l)) => layout = Some(l),
                    (Some(a), Some(b)) if a == b => {}
                    _ => continue 'containers, // streams disagree: not something we can drive
                }
            }
            chosen = layout.map(|l| (container, l));
            break;
        }
        let Some((container, layout)) = chosen else {
            unwind(self);
            return Err(MusicError::Audio(
                "device accepts neither a 24-bit nor a 32-bit integer stream format; refusing DoP"
                    .into(),
            ));
        };
        self.container = container;
        self.layout = layout;

        self.device = device;
        self.active_rate = rate;
        self.channels = channels;
        Ok(())
    }
}

/// How long to wait for a device to report a rate change. USB DACs apply it
/// asynchronously (the K15 takes several hundred milliseconds), so an
/// immediate readback still shows the old rate.
const RATE_SETTLE: Duration = Duration::from_millis(2500);

/// Set the nominal rate and wait until the device reports it (bounded).
fn set_rate_and_wait(device: AudioObjectID, rate: f64) -> bool {
    if set_nominal_rate(device, rate).is_err() {
        return false;
    }
    let deadline = Instant::now() + RATE_SETTLE;
    loop {
        if matches!(get_nominal_rate(device), Ok(r) if (r - rate).abs() < 1.0) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Put a device back how `acquire` found it: each stream's original physical
/// format first (that is what pins the rate), then the nominal rate.
fn restore_device(device: AudioObjectID, rate: f64, formats: &[SavedFormat]) {
    for (stream, physical, virt) in formats {
        let _ = set_stream_format(*stream, physical);
        let _ = set_virtual_format(*stream, virt);
    }
    if rate > 0.0 {
        // Wait for it: the next open records the device's rate as "original",
        // and must not catch it mid-switch.
        let _ = set_rate_and_wait(device, rate);
    }
}

impl Default for CoreAudioDopSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CoreAudioDopSink {
    fn drop(&mut self) {
        self.release();
    }
}

impl CoreAudioDopSink {
    /// Can the running session carry a new stream at `rate`/`channels` of
    /// this kind? If so, flush it and return true. Only a session whose IO is
    /// running qualifies: the flush is acknowledged by the render thread,
    /// and only then is it safe to write (else fresh audio could be flushed).
    fn reuse_session(&mut self, rate: u32, channels: u16, silence: Silence) -> bool {
        if self.device == 0
            || self.io_proc.is_none()
            || self.producer.is_none()
            || self.state != SinkState::Playing
            || self.active_rate != rate
            || self.channels != channels
            || self.silence != silence
        {
            return false;
        }
        self.flush.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_millis(200);
        while self.flush.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                self.flush.store(false, Ordering::Release);
                return false; // render thread not answering: start clean
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    /// Acquire the device at `rate` / `channels` and create the IO proc.
    /// Shared by the DoP path (`open`) and exclusive PCM (`open_exclusive_pcm`).
    fn open_at(&mut self, rate: u32, channels: u16, silence: Silence) -> Result<(), MusicError> {
        self.acquire(rate, channels)?;

        // DoP buffers two seconds; PCM one (position is corrected by the
        // buffered amount, so latency is bookkeeping, not drift).
        let seconds = if silence == Silence::Dop { 2 } else { 1 };
        let cap = (seconds * rate as usize * channels as usize * 3).min(RING_CAP_MAX);
        let (prod, cons) = HeapRb::<u8>::new(cap).split();
        let render_state = Box::new(RenderState {
            cons,
            underruns: self.underruns.clone(),
            channels,
            layout: self.layout,
            flush: self.flush.clone(),
            marker: MARKER_EVEN,
            silence,
        });
        let raw = Box::into_raw(render_state);
        let mut proc_id: AudioDeviceIOProcID = None;
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                self.device,
                Some(dop_io_proc),
                raw as *mut c_void,
                &mut proc_id,
            )
        };
        if status != NO_ERR || proc_id.is_none() {
            unsafe {
                let _ = Box::from_raw(raw);
            }
            let e = MusicError::Audio(format!(
                "CoreAudio IO proc create failed: OSStatus {status}"
            ));
            self.release();
            return Err(e);
        }
        self.silence = silence;
        self.flush.store(false, Ordering::Release);
        self.render_state = raw;
        self.io_proc = Some(proc_id);
        self.producer = Some(prod);
        self.state = SinkState::Stopped;
        Ok(())
    }
}

impl AudioSink for CoreAudioDopSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        let dsd_rate = track
            .sample_rate
            .ok_or_else(|| MusicError::Audio("DoP needs a known DSD rate".into()))?;
        let rate = dop_pcm_rate(dsd_rate)
            .ok_or_else(|| MusicError::Audio(format!("not a DSD rate: {dsd_rate}")))?;
        let channels = track.channels.unwrap_or(2).clamp(1, 8) as u16;
        // Same rate and channels as the running session (next album track, a
        // seek): keep the device hogged, locked and streaming valid DoP
        // silence, and only drop the stale audio. No re-lock click, no gap
        // while the device is re-acquired.
        if self.reuse_session(rate, channels, Silence::Dop) {
            return Ok(());
        }
        self.release();
        self.open_at(rate, channels, Silence::Dop)
    }

    fn exclusive_pcm_rate(&self, rate_hz: u32) -> Option<u32> {
        // Capability query only: never changes device state.
        let device = resolve_device(self.device_name.as_deref()).ok()?;
        let avail = available_sample_rates(device).ok()?;
        avail
            .iter()
            .any(|a| (*a - rate_hz as f64).abs() < 1.0)
            .then_some(rate_hz)
    }

    fn open_exclusive_pcm(&mut self, rate_hz: u32, channels: u16) -> Result<(), MusicError> {
        // Next track at the same rate and channel count: keep the device
        // hogged, locked and streaming zeros, and only drop stale audio. No
        // ~1.7 s re-acquire gap, and no device reconfiguration between tracks.
        if self.reuse_session(rate_hz, channels.clamp(1, 8), Silence::Pcm) {
            return Ok(());
        }
        self.release();
        if self.exclusive_pcm_rate(rate_hz).is_none() {
            return Err(MusicError::Audio(format!(
                "output device does not offer {rate_hz} Hz"
            )));
        }
        self.open_at(rate_hz, channels.clamp(1, 8), Silence::Pcm)
    }

    fn buffered_frames(&self) -> u64 {
        match (&self.producer, self.channels) {
            (Some(p), ch) if ch > 0 => (p.occupied_len() / (ch as usize * 3)) as u64,
            _ => 0,
        }
    }

    fn drain(&mut self) {
        // Only a running device empties its ring.
        if self.state != SinkState::Playing {
            return;
        }
        let Some(p) = self.producer.as_ref() else {
            return;
        };
        let frame_bytes = (self.channels as usize * 3).max(1);
        let rate = u64::from(self.active_rate.max(1));
        let queued_ms = (p.occupied_len() / frame_bytes) as u64 * 1000 / rate;
        let deadline = Instant::now() + Duration::from_millis(queued_ms + 500);
        while p.occupied_len() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn write(&mut self, _chunk: PcmChunk) -> Result<(), MusicError> {
        Err(MusicError::Audio(
            "DoP sink does not accept PCM (bit-perfect path)".into(),
        ))
    }

    fn write_dop(&mut self, frames: &[u8]) -> Result<(), MusicError> {
        let prod = self
            .producer
            .as_mut()
            .ok_or_else(|| MusicError::Audio("DoP sink not open".into()))?;
        debug_assert_eq!(frames.len() % (self.channels as usize * 3), 0);
        let mut rest = frames;
        let deadline = Instant::now() + WRITE_DEADLINE;
        while !rest.is_empty() {
            let n = prod.push_slice(rest);
            rest = &rest[n..];
            if !rest.is_empty() {
                if Instant::now() >= deadline {
                    return Err(MusicError::Audio(
                        "DoP ring full: device not draining".into(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(())
    }

    fn play(&mut self) -> Result<(), MusicError> {
        let proc = self
            .io_proc
            .ok_or_else(|| MusicError::Audio("DoP sink not open".into()))?;
        if self.state == SinkState::Playing {
            return Ok(()); // a reused session is already running
        }
        os(
            unsafe { AudioDeviceStart(self.device, proc) },
            "start DoP IO",
        )?;
        self.state = SinkState::Playing;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), MusicError> {
        let proc = self
            .io_proc
            .ok_or_else(|| MusicError::Audio("DoP sink not open".into()))?;
        os(unsafe { AudioDeviceStop(self.device, proc) }, "stop DoP IO")?;
        self.state = SinkState::Paused;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MusicError> {
        // Full release: another app can use the device between albums.
        self.release();
        Ok(())
    }

    fn state(&self) -> SinkState {
        self.state
    }

    fn supports_dop(&self) -> bool {
        true
    }

    fn dop_output_rate(&self, dsd_rate_hz: u32) -> Option<u32> {
        // Capability query only: never changes device state.
        let rate = dop_pcm_rate(dsd_rate_hz)?;
        let device = resolve_device(self.device_name.as_deref()).ok()?;
        let avail = available_sample_rates(device).ok()?;
        let rate_ok = avail.iter().any(|a| (*a - rate as f64).abs() < 1.0);
        // The rate alone is not enough: the device must also offer an
        // integer container we can drive at that rate.
        (rate_ok && !offered_containers(device, rate, 2).is_empty()).then_some(rate)
    }

    fn select_output_path(&mut self, _path: OutputPath) {
        // This sink *is* the DoP path.
    }

    fn output_device_name(&self) -> Option<String> {
        resolved_device_name(self.device_name.as_deref())
    }

    fn output_is_external_dac(&self) -> bool {
        resolve_device(self.device_name.as_deref())
            .map(|d| is_external_transport(transport_name(d)))
            .unwrap_or(false)
    }

    fn set_output_device(&mut self, name: Option<&str>) {
        let name = name.map(str::to_owned);
        if name != self.device_name {
            // Give the old device back (hog mode off, nominal rate restored)
            // before switching; the next open() acquires the new one.
            self.release();
            self.device_name = name;
        }
    }

    fn underrun_count(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;

    /// Manual: `KAHAWAI_DOP_DEVICE="FIIO K15 " cargo test -p kahawai-player-audio
    /// -- --ignored --nocapture k15`. Takes hog mode and sets the DoP rate and
    /// stream format, then releases; the IO proc is never started, so no audio
    /// reaches the device.
    #[test]
    #[ignore]
    fn dop_format_negotiation_on_a_real_device() {
        let name = std::env::var("KAHAWAI_DOP_DEVICE").ok();
        let mut sink = CoreAudioDopSink::new();
        sink.set_output_device(name.as_deref());
        let device = resolve_device(name.as_deref()).unwrap();
        println!("capabilities: {:?}", device_capabilities(name.as_deref()));
        println!("live (idle): {:?}", device_live_state(name.as_deref()));
        println!("offered @176.4k: {:?}", offered_containers(device, 176_400, 2));
        println!("dop_output_rate(DSD64): {:?}", sink.dop_output_rate(2_822_400));
        sink.open_at(176_400, 2, Silence::Dop).expect("acquire");
        println!("negotiated container: {:?}, io layout: {:?}", sink.container, sink.layout);
        println!("live (exclusive): {:?}", device_live_state(name.as_deref()));
        let before = sink.saved_rate;
        sink.release();
        // The device must come back to the rate it had (and stay unhogged).
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut now = get_nominal_rate(device).unwrap();
        while (now - before).abs() > 1.0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            now = get_nominal_rate(device).unwrap();
        }
        println!("rate restored: {before} -> {now}");
        assert!((now - before).abs() < 1.0, "device left at {now} Hz, expected {before}");
        assert_eq!(get_hog_pid(device).unwrap(), -1, "hog mode released");
    }
}

#[cfg(test)]
mod cycle_tests {
    use super::*;

    /// Manual: `KAHAWAI_DOP_DEVICE="FIIO K15 " cargo test -p kahawai-player-audio
    /// -- --ignored --nocapture exclusive_pcm_cycles`. Opens and releases the
    /// exclusive PCM session back to back, like consecutive tracks, running the
    /// IO proc on an empty ring (all-zero output: silent), and prints what the
    /// device reports each time.
    #[test]
    #[ignore]
    fn exclusive_pcm_cycles_on_a_real_device() {
        let name = std::env::var("KAHAWAI_DOP_DEVICE").ok();
        let mut sink = CoreAudioDopSink::new();
        sink.set_output_device(name.as_deref());
        let device = resolve_device(name.as_deref()).unwrap();
        for i in 0..4 {
            let t0 = Instant::now();
            sink.open_exclusive_pcm(44_100, 2).expect("open exclusive pcm");
            sink.play().expect("play");
            println!("cycle {i}: opened+started in {:?}, container {:?}, io layout {:?}", t0.elapsed(), sink.container, sink.layout);
            for ms in [0u64, 100, 300, 600] {
                std::thread::sleep(Duration::from_millis(if ms == 0 { 0 } else { 100 }));
                let live = device_live_state(name.as_deref()).unwrap();
                let fmt = output_streams(device).ok().and_then(|s| s.first().copied()).and_then(|s| get_stream_format(s).ok());
                // The IO proc's buffers use the VIRTUAL format, not the physical one.
                let virt = output_streams(device).ok().and_then(|s| s.first().copied()).and_then(|s| {
                    let addr = prop_addr(kAudioStreamPropertyVirtualFormat);
                    let mut a: AudioStreamBasicDescription = unsafe { std::mem::zeroed() };
                    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
                    let st = unsafe {
                        AudioObjectGetPropertyData(s, &addr, 0, ptr::null(), &mut size, &mut a as *mut _ as *mut c_void)
                    };
                    (st == NO_ERR).then_some(a)
                });
                let v = virt.expect("virtual format readable");
                assert!(
                    classify_virtual(&v, 44_100, 2, sink.container) == Some(sink.layout),
                    "cycle {i}: the IO callback's buffers are not what we render (virtual bits {}, flags {:#x}, layout {:?})",
                    v.mBitsPerChannel,
                    v.mFormatFlags,
                    sink.layout
                );
                println!(
                    "   +{ms}ms rate={} PHYS bits={} bpf={:?} flags={:#x} | VIRT bits={:?} bpf={:?} flags={:#x?} float={:?}",
                    live.rate_hz,
                    live.bit_depth,
                    fmt.map(|f| f.mBytesPerFrame),
                    fmt.map(|f| f.mFormatFlags).unwrap_or(0),
                    virt.map(|v| v.mBitsPerChannel),
                    virt.map(|v| v.mBytesPerFrame),
                    virt.map(|v| v.mFormatFlags),
                    virt.map(|v| v.mFormatFlags & kAudioFormatFlagIsFloat != 0),
                );
            }
            sink.stop().unwrap();
        }
        // Consecutive same-rate tracks: the session is reused, not re-acquired.
        sink.open_exclusive_pcm(44_100, 2).expect("first track");
        sink.play().expect("play");
        std::thread::sleep(Duration::from_millis(200));
        let t0 = Instant::now();
        sink.open_exclusive_pcm(44_100, 2).expect("second track");
        sink.play().expect("play");
        let took = t0.elapsed();
        println!("second same-rate track opened in {took:?} (layout {:?})", sink.layout);
        assert!(took < Duration::from_millis(400), "session reused, not re-acquired: {took:?}");
        assert_eq!(get_hog_pid(device).unwrap(), std::process::id() as i32, "still hogged throughout");
        sink.stop().unwrap();
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// The names the UI offers come from cpal; the DoP sink must resolve
    /// the same names to HAL devices (or the DAC choice would silently
    /// fall back to the default). Runs against whatever devices this
    /// machine has; trivially passes with none.
    #[test]
    fn every_cpal_output_device_resolves_by_name() {
        let devices = crate::list_output_devices();
        eprintln!(
            "checking {} output device(s): {:?}",
            devices.len(),
            devices.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
        for d in devices {
            let id = find_output_device(&d.name);
            assert!(
                id.is_some(),
                "no HAL device found for cpal name {:?}",
                d.name
            );
            assert_eq!(device_name_of(id.unwrap()).as_deref(), Some(d.name.as_str()));
        }
    }

    #[test]
    fn unknown_device_falls_back_to_default_and_missing_name_is_none() {
        assert!(find_output_device("definitely not a real device \u{1F50A}").is_none());
        if let Ok(default) = default_output_device() {
            assert_eq!(
                resolve_device(Some("definitely not a real device")).unwrap(),
                default
            );
            assert_eq!(resolve_device(None).unwrap(), default);
        }
    }

    #[test]
    fn dop_rates_query_accepts_a_named_device() {
        // Must not panic/err for any listed device or an unknown name.
        for d in crate::list_output_devices() {
            let _ = supported_dop_rates(Some(&d.name));
        }
        let _ = supported_dop_rates(Some("nope"));
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use ringbuf::traits::{Consumer, Producer};

    fn state(silence: Silence, channels: u16, ring: usize) -> (RenderState, ringbuf::HeapProd<u8>) {
        let (prod, cons) = HeapRb::<u8>::new(ring).split();
        (
            RenderState {
                cons,
                underruns: Arc::new(AtomicU64::new(0)),
                channels,
                layout: IoLayout::Packed24,
                flush: Arc::new(AtomicBool::new(false)),
                marker: MARKER_EVEN,
                silence,
            },
            prod,
        )
    }

    #[test]
    fn a_flush_request_drops_stale_audio_and_is_acknowledged() {
        let (mut st, mut prod) = state(Silence::Dop, 2, 4096);
        prod.push_slice(&[0x11, 0x22, MARKER_EVEN, 0x33, 0x44, MARKER_ODD]);
        st.flush.store(true, Ordering::Release);
        let mut out = vec![0u8; 6];
        render_into(&mut st, &mut out);
        assert!(!st.flush.load(Ordering::Acquire), "acknowledged: safe to write again");
        assert_eq!(
            out,
            [DSD_SILENCE, DSD_SILENCE, MARKER_EVEN, DSD_SILENCE, DSD_SILENCE, MARKER_EVEN],
            "the stale frame never reaches the device; valid DoP silence instead"
        );
        // Audio written after the acknowledgement plays normally.
        prod.push_slice(&[0xAA, 0xBB, MARKER_ODD, 0xCC, 0xDD, MARKER_ODD]);
        let mut out2 = vec![0u8; 6];
        render_into(&mut st, &mut out2);
        assert_eq!(out2, [0xAA, 0xBB, MARKER_ODD, 0xCC, 0xDD, MARKER_ODD]);
    }

    #[test]
    fn a_32_bit_container_carries_the_same_24_bits_left_aligned() {
        let (mut st, mut prod) = state(Silence::Dop, 2, 4096);
        st.layout = IoLayout::Int32;
        prod.push_slice(&[0x11, 0x22, MARKER_EVEN, 0x33, 0x44, MARKER_EVEN]); // one stereo frame
        let mut out = vec![0xEEu8; 8];
        render_into(&mut st, &mut out);
        assert_eq!(
            out,
            [0, 0x11, 0x22, MARKER_EVEN, 0, 0x33, 0x44, MARKER_EVEN],
            "marker stays the most significant byte, low byte is zero"
        );
    }

    #[test]
    fn a_float32_io_layout_scales_each_24_bit_sample_by_exactly_two_to_the_minus_23() {
        let (mut st, mut prod) = state(Silence::Pcm, 2, 4096);
        st.layout = IoLayout::Float32;
        // L = +1 (0x000001), R = -1 (0xFFFFFF); then full-scale positive and negative.
        prod.push_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x80]);
        let mut out = vec![0u8; 16];
        render_into(&mut st, &mut out);
        let f = |i: usize| f32::from_le_bytes([out[i * 4], out[i * 4 + 1], out[i * 4 + 2], out[i * 4 + 3]]);
        assert_eq!(f(0), 1.0 / 8_388_608.0);
        assert_eq!(f(1), -1.0 / 8_388_608.0);
        assert_eq!(f(2), 8_388_607.0 / 8_388_608.0);
        assert_eq!(f(3), -1.0);
    }

    #[test]
    fn float32_round_trips_every_24_bit_word_exactly() {
        // What the driver does with the float: scale by 2^31. It must give the
        // original 24-bit word back, shifted left 8, for the whole range.
        for v in (-8_388_608i32..8_388_608).step_by(4099).chain([-8_388_608, -1, 0, 1, 8_388_607]) {
            let f = v as f32 / 8_388_608.0;
            assert_eq!((f as f64 * 2_147_483_648.0) as i64, (v as i64) << 8, "word {v}");
        }
    }

    #[test]
    fn dop_markers_survive_the_float32_layout_exactly() {
        let (mut st, mut prod) = state(Silence::Dop, 2, 4096);
        st.layout = IoLayout::Float32;
        prod.push_slice(&[0x12, 0x34, MARKER_EVEN, 0x12, 0x34, MARKER_EVEN, 0x56, 0x78, MARKER_ODD, 0x56, 0x78, MARKER_ODD]);
        let mut out = vec![0u8; 16];
        render_into(&mut st, &mut out);
        let word = |i: usize| {
            let f = f32::from_le_bytes([out[i * 4], out[i * 4 + 1], out[i * 4 + 2], out[i * 4 + 3]]);
            ((f as f64 * 2_147_483_648.0) as i64 >> 8) as i32
        };
        assert_eq!(word(0) & 0xFF_FFFF, 0x05_3412, "marker 0x05 in the top byte, payload intact");
        assert_eq!((word(2) as u32 >> 16) & 0xFF, 0xFA, "marker 0xFA survives the sign");
    }

    #[test]
    fn a_32_bit_container_underruns_still_alternate_markers() {
        let (mut st, _prod) = state(Silence::Dop, 2, 4096);
        st.layout = IoLayout::Int32;
        let mut out = vec![0u8; 16]; // 2 frames
        render_into(&mut st, &mut out);
        assert_eq!(out[3], MARKER_EVEN);
        assert_eq!(out[11], MARKER_ODD);
        assert_eq!(st.underruns.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn pcm_underruns_are_true_silence_not_dop_bytes() {
        let mut marker = MARKER_EVEN;
        let mut frame = [0xFFu8; 6];
        fill_underrun_frame(&mut frame, 2, Silence::Pcm, &mut marker);
        assert_eq!(
            frame, [0u8; 6],
            "a DoP pattern here would be a loud burst on a PCM stream"
        );
        assert_eq!(marker, MARKER_EVEN, "PCM does not touch the DoP marker");
    }

    #[test]
    fn dop_underruns_keep_valid_alternating_markers() {
        let mut marker = MARKER_EVEN;
        let mut a = [0u8; 6];
        let mut b = [0u8; 6];
        fill_underrun_frame(&mut a, 2, Silence::Dop, &mut marker);
        fill_underrun_frame(&mut b, 2, Silence::Dop, &mut marker);
        assert_eq!(
            a,
            [
                DSD_SILENCE,
                DSD_SILENCE,
                MARKER_EVEN,
                DSD_SILENCE,
                DSD_SILENCE,
                MARKER_EVEN
            ]
        );
        assert_eq!(
            b,
            [
                DSD_SILENCE,
                DSD_SILENCE,
                MARKER_ODD,
                DSD_SILENCE,
                DSD_SILENCE,
                MARKER_ODD
            ]
        );
    }

    #[test]
    fn pcm_frames_reach_the_device_byte_for_byte() {
        let (mut st, mut prod) = state(Silence::Pcm, 2, 4096);
        let src: Vec<u8> = (0..24u8)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect(); // 4 stereo frames
        assert_eq!(prod.push_slice(&src), 24);
        let mut out = vec![0xEEu8; 24];
        render_into(&mut st, &mut out);
        assert_eq!(out, src, "no scaling, no marker stamping, no reordering");
        assert_eq!(st.underruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_short_pcm_ring_is_completed_with_zeros_and_counted() {
        let (mut st, mut prod) = state(Silence::Pcm, 2, 4096);
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]; // 2 frames
        prod.push_slice(&src);
        let mut out = vec![0xEEu8; 24]; // 4 frames requested
        render_into(&mut st, &mut out);
        assert_eq!(&out[..12], &src);
        assert_eq!(&out[12..], &[0u8; 12], "underrun frames are silent");
        assert_eq!(
            st.underruns.load(Ordering::Relaxed),
            2,
            "one per missing frame"
        );
    }

    #[test]
    fn dop_rendering_is_unchanged_by_the_pcm_mode() {
        let (mut st, mut prod) = state(Silence::Dop, 2, 4096);
        prod.push_slice(&[0x11, 0x22, MARKER_EVEN, 0x33, 0x44, MARKER_EVEN, 0x55, 0x66, MARKER_ODD, 0x77, 0x88, MARKER_ODD]);
        let mut out = vec![0u8; 12];
        render_into(&mut st, &mut out);
        assert_eq!(
            out,
            [0x11, 0x22, MARKER_EVEN, 0x33, 0x44, MARKER_EVEN, 0x55, 0x66, MARKER_ODD, 0x77, 0x88, MARKER_ODD],
            "an already-correct stream passes through untouched"
        );
    }

    /// Every marker in a rendered DoP buffer, per frame (stereo, packed 24).
    fn markers(out: &[u8]) -> Vec<u8> {
        out.chunks(6).map(|f| f[2]).collect()
    }

    #[test]
    fn a_new_stream_after_silence_keeps_the_marker_alternation_unbroken() {
        // The bug: between tracks the device plays the renderer's own silence
        // frames; the next track's first frame carried the same marker as the
        // last silence frame, breaking the alternation (the DAC then plays the
        // DoP as noise).
        let (mut st, mut prod) = state(Silence::Dop, 2, 4096);
        let mut silence = vec![0u8; 6 * 3]; // three underrun frames: EVEN, ODD, EVEN
        render_into(&mut st, &mut silence);
        assert_eq!(markers(&silence), [MARKER_EVEN, MARKER_ODD, MARKER_EVEN]);
        // The next track starts on EVEN again (its own parity), which would repeat.
        prod.push_slice(&[0xAA, 0xBB, MARKER_EVEN, 0xAA, 0xBB, MARKER_EVEN, 0xCC, 0xDD, MARKER_ODD, 0xCC, 0xDD, MARKER_ODD]);
        let mut out = vec![0u8; 12];
        render_into(&mut st, &mut out);
        assert_eq!(markers(&out), [MARKER_ODD, MARKER_EVEN], "continues the alternation");
        assert_eq!(&out[..2], &[0xAA, 0xBB], "the DSD payload is never touched");
        assert_eq!(&out[6..8], &[0xCC, 0xDD]);
    }

    #[test]
    fn markers_alternate_across_tracks_of_odd_length_and_underruns() {
        let (mut st, mut prod) = state(Silence::Dop, 2, 4096);
        let mut all = Vec::new();
        for track in 0..3u8 {
            // 3 frames per "track", each starting on EVEN: odd length, so the
            // naive concatenation repeats a marker at every boundary.
            for f in 0..3u8 {
                let m = if f % 2 == 0 { MARKER_EVEN } else { MARKER_ODD };
                prod.push_slice(&[track, f, m, track, f, m]);
            }
            let mut out = vec![0u8; 6 * 4]; // ask for one more frame than exists: an underrun
            render_into(&mut st, &mut out);
            all.extend_from_slice(&out);
        }
        let m = markers(&all);
        assert!(
            m.windows(2).all(|w| w[0] != w[1]),
            "markers must strictly alternate across boundaries and underruns: {m:02X?}"
        );
    }

    #[test]
    fn all_channels_share_the_same_marker_in_a_frame() {
        let (mut st, mut prod) = state(Silence::Dop, 2, 4096);
        prod.push_slice(&[1, 2, MARKER_ODD, 3, 4, MARKER_ODD]); // wrong phase on purpose
        let mut out = vec![0u8; 6];
        render_into(&mut st, &mut out);
        assert_eq!(out[2], out[5]);
        assert_eq!(out[2], MARKER_EVEN, "the first frame of a session starts the sequence");
    }

    #[test]
    fn a_pcm_stream_is_never_stamped() {
        let (mut st, mut prod) = state(Silence::Pcm, 2, 4096);
        prod.push_slice(&[1, 2, 0x77, 3, 4, 0x77]);
        let mut out = vec![0u8; 6];
        render_into(&mut st, &mut out);
        assert_eq!(out, [1, 2, 0x77, 3, 4, 0x77], "only DoP has markers");
    }

    #[test]
    fn a_partial_trailing_frame_is_never_split() {
        let (mut st, mut prod) = state(Silence::Pcm, 2, 64);
        prod.push_slice(&[9u8; 6]);
        let mut out = vec![0xEEu8; 8]; // one whole frame + 2 stray bytes
        render_into(&mut st, &mut out);
        assert_eq!(&out[..6], &[9u8; 6]);
        assert_eq!(
            &out[6..],
            &[0xEE, 0xEE],
            "bytes beyond a whole frame are left alone"
        );
    }

    // -- buffered-frame accounting and drain ---------------------------------

    fn open_ring(
        channels: u16,
        rate: u32,
        bytes: usize,
    ) -> (CoreAudioDopSink, ringbuf::HeapCons<u8>) {
        let mut sink = CoreAudioDopSink::new();
        let (mut prod, cons) = HeapRb::<u8>::new(1 << 20).split();
        prod.push_slice(&vec![0u8; bytes]);
        sink.producer = Some(prod);
        sink.channels = channels;
        sink.active_rate = rate;
        (sink, cons)
    }

    #[test]
    fn buffered_frames_counts_whole_24_bit_frames_and_is_zero_when_closed() {
        let closed = CoreAudioDopSink::new();
        assert_eq!(closed.buffered_frames(), 0);
        let (sink, _cons) = open_ring(2, 96_000, 6 * 1000);
        assert_eq!(sink.buffered_frames(), 1000, "6 bytes per stereo frame");
        let (six, _c) = open_ring(6, 96_000, 18 * 50);
        assert_eq!(six.buffered_frames(), 50);
    }

    #[test]
    fn drain_waits_for_the_device_to_empty_the_ring_then_returns() {
        let (mut sink, mut cons) = open_ring(2, 44_100, 6 * 2000);
        sink.state = SinkState::Playing;
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            let mut buf = vec![0u8; 6 * 2000];
            cons.pop_slice(&mut buf); // the device catches up
        });
        let start = Instant::now();
        sink.drain();
        assert_eq!(
            sink.buffered_frames(),
            0,
            "everything queued was played out"
        );
        assert!(
            start.elapsed() >= Duration::from_millis(40),
            "it actually waited"
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        t.join().unwrap();
    }

    #[test]
    fn drain_gives_up_after_its_bound_when_the_device_never_drains() {
        let (mut sink, _cons) = open_ring(2, 44_100, 6 * 10); // 10 frames, nobody consuming
        sink.state = SinkState::Playing;
        let start = Instant::now();
        sink.drain();
        assert!(
            start.elapsed() < Duration::from_millis(1500),
            "bounded: queued time + 500 ms slack"
        );
        assert_eq!(sink.buffered_frames(), 10);
    }

    #[test]
    fn drain_does_not_wait_on_a_paused_or_closed_sink() {
        let (mut sink, _cons) = open_ring(2, 44_100, 6 * 1000);
        sink.state = SinkState::Paused;
        let start = Instant::now();
        sink.drain();
        assert!(start.elapsed() < Duration::from_millis(50));
        let mut closed = CoreAudioDopSink::new();
        closed.drain();
    }

    #[test]
    fn exclusive_pcm_is_refused_for_a_rate_no_device_offers() {
        let mut sink = CoreAudioDopSink::new();
        // No real device offers 1 Hz: the capability query says no, and opening
        // fails cleanly without touching hog mode or any device state.
        assert_eq!(sink.exclusive_pcm_rate(1), None);
        assert!(sink.open_exclusive_pcm(1, 2).is_err());
        assert_eq!(sink.state(), SinkState::Stopped);
    }
}
