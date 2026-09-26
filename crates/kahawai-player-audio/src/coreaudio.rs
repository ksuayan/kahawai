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
//! 3. **Stream physical format** forced to 24-bit packed signed-integer
//!    PCM and verified by readback — CoreAudio HAL streams default to
//!    Float32, which would destroy the DoP marker encoding.
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
use std::sync::atomic::{AtomicU64, Ordering};
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

fn dop_stream_format(rate: u32, channels: u16) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: rate as f64,
        mFormatID: kAudioFormatLinearPCM,
        // Packed signed-integer, native (little) endian: the DoP bytes
        // land on the wire exactly as the server packed them.
        mFormatFlags: kAudioFormatFlagIsPacked | kAudioFormatFlagIsSignedInteger,
        mBytesPerPacket: channels as u32 * 3,
        mFramesPerPacket: 1,
        mBytesPerFrame: channels as u32 * 3,
        mChannelsPerFrame: channels as u32,
        mBitsPerChannel: 24,
        mReserved: 0,
    }
}

fn set_stream_format(
    stream: AudioStreamID,
    asbd: &AudioStreamBasicDescription,
) -> Result<(), MusicError> {
    let addr = prop_addr(kAudioStreamPropertyPhysicalFormat);
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
        "set stream physical format",
    )
}

fn get_stream_format(stream: AudioStreamID) -> Result<AudioStreamBasicDescription, MusicError> {
    let addr = prop_addr(kAudioStreamPropertyPhysicalFormat);
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
        "get stream physical format",
    )?;
    Ok(asbd)
}

/// State owned by the IO proc: the render thread only touches the
/// consumer half of the ring, the engine thread only the producer half.
struct RenderState {
    cons: HeapCons<u8>,
    underruns: Arc<AtomicU64>,
    channels: u16,
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
    let frame_bytes = state.channels as usize * 3;
    if frame_bytes == 0 {
        return;
    }
    let frames = bytes.len() / frame_bytes;
    for f in 0..frames {
        let dst = &mut bytes[f * frame_bytes..(f + 1) * frame_bytes];
        if state.cons.pop_slice(dst) == frame_bytes {
            continue;
        }
        // Underrun: one frame of the stream's own kind of silence (DoP
        // keeps its markers alternating so the DAC never loses sync; PCM
        // gets zeros).
        state.underruns.fetch_add(1, Ordering::Relaxed);
        fill_underrun_frame(dst, state.channels, state.silence, &mut state.marker);
    }
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
fn device_name(device: AudioObjectID) -> Option<String> {
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
        device_name(d).as_deref() == Some(name)
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

/// DoP rates the given output device (`None` = system default) reports as
/// available nominal rates.
pub fn supported_dop_rates(device_name: Option<&str>) -> Result<Vec<u32>, MusicError> {
    let device = resolve_device(device_name)?;
    let avail = available_sample_rates(device)?;
    Ok(DOP_RATES
        .into_iter()
        .filter(|r| avail.iter().any(|a| (*a - *r as f64).abs() < 1.0))
        .collect())
}

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
    saved_rate: f64,
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
            saved_rate: 0.0,
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
                if self.saved_rate > 0.0 {
                    let _ = set_nominal_rate(self.device, self.saved_rate);
                }
                // -1 = no owner: release hog mode.
                let _ = set_hog_pid(self.device, -1);
            }
        }
        self.io_proc = None;
        self.producer = None;
        self.device = 0;
        self.active_rate = 0;
        self.channels = 0;
        self.saved_rate = 0.0;
        self.state = SinkState::Stopped;
    }

    /// Acquire hog mode, switch to `rate`, force 24-bit stream formats.
    /// Every step is verified by readback; any failure unwinds.
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

        // 2. Exact sample rate, verified by readback.
        let mut ok = false;
        let undo = |device: AudioObjectID| {
            let _ = set_hog_pid(device, -1);
        };
        if set_nominal_rate(device, rate as f64).is_ok() {
            if let Ok(back) = get_nominal_rate(device) {
                ok = (back - rate as f64).abs() < 1.0;
            }
        }
        if !ok {
            undo(device);
            return Err(MusicError::Audio(format!(
                "device would not switch to {rate} Hz; refusing DoP"
            )));
        }

        // 3. 24-bit physical stream format on every output stream,
        // verified by readback. (HAL streams default to Float32, which
        // would destroy the DoP encoding.)
        let asbd = dop_stream_format(rate, channels);
        let streams = output_streams(device)?;
        if streams.is_empty() {
            undo(device);
            let _ = set_nominal_rate(device, self.saved_rate);
            return Err(MusicError::Audio("device has no output streams".into()));
        }
        for s in &streams {
            let fail = |device: AudioObjectID| {
                let _ = set_nominal_rate(device, self.saved_rate);
                let _ = set_hog_pid(device, -1);
            };
            if set_stream_format(*s, &asbd).is_err() {
                fail(device);
                return Err(MusicError::Audio(
                    "device refused the 24-bit DoP stream format".into(),
                ));
            }
            match get_stream_format(*s) {
                Ok(back)
                    if (back.mSampleRate - rate as f64).abs() < 1.0
                        && back.mBitsPerChannel == 24
                        && back.mChannelsPerFrame == channels as u32
                        && back.mBytesPerFrame == channels as u32 * 3 =>
                {
                    // verified
                }
                _ => {
                    fail(device);
                    return Err(MusicError::Audio(
                        "24-bit DoP stream format did not stick; refusing DoP".into(),
                    ));
                }
            }
        }

        self.device = device;
        self.active_rate = rate;
        self.channels = channels;
        Ok(())
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
        self.render_state = raw;
        self.io_proc = Some(proc_id);
        self.producer = Some(prod);
        self.state = SinkState::Stopped;
        Ok(())
    }
}

impl AudioSink for CoreAudioDopSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        self.release();
        let dsd_rate = track
            .sample_rate
            .ok_or_else(|| MusicError::Audio("DoP needs a known DSD rate".into()))?;
        let rate = dop_pcm_rate(dsd_rate)
            .ok_or_else(|| MusicError::Audio(format!("not a DSD rate: {dsd_rate}")))?;
        let channels = track.channels.unwrap_or(2).clamp(1, 8) as u16;
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
        avail
            .iter()
            .any(|a| (*a - rate as f64).abs() < 1.0)
            .then_some(rate)
    }

    fn select_output_path(&mut self, _path: OutputPath) {
        // This sink *is* the DoP path.
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
            assert_eq!(device_name(id.unwrap()).as_deref(), Some(d.name.as_str()));
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
                marker: MARKER_EVEN,
                silence,
            },
            prod,
        )
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
        prod.push_slice(&[0x11, 0x22, MARKER_EVEN, 0x33, 0x44, MARKER_EVEN]);
        let mut out = vec![0u8; 12];
        render_into(&mut st, &mut out);
        assert_eq!(
            &out[..6],
            &[0x11, 0x22, MARKER_EVEN, 0x33, 0x44, MARKER_EVEN]
        );
        assert_eq!(
            &out[6..],
            &[
                DSD_SILENCE,
                DSD_SILENCE,
                MARKER_EVEN,
                DSD_SILENCE,
                DSD_SILENCE,
                MARKER_EVEN
            ]
        );
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
