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
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut device as *mut _ as *mut c_void,
        ),
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
        AudioObjectGetPropertyDataSize(device, &addr, 0, ptr::null(), &mut size),
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
        AudioObjectGetPropertyData(
            device,
            &addr,
            0,
            ptr::null(),
            &mut size2,
            ranges.as_mut_ptr() as *mut c_void,
        ),
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
        AudioObjectGetPropertyData(
            device,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut rate as *mut _ as *mut c_void,
        ),
        "get nominal sample rate",
    )?;
    Ok(rate)
}

fn set_nominal_rate(device: AudioObjectID, rate: f64) -> Result<(), MusicError> {
    let addr = prop_addr(kAudioDevicePropertyNominalSampleRate);
    let rate = rate;
    os(
        AudioObjectSetPropertyData(
            device,
            &addr,
            0,
            ptr::null(),
            std::mem::size_of::<f64>() as u32,
            &rate as *const _ as *const c_void,
        ),
        "set nominal sample rate",
    )
}

fn get_hog_pid(device: AudioObjectID) -> Result<i32, MusicError> {
    let addr = prop_addr(kAudioDevicePropertyHogMode);
    let mut pid: i32 = 0;
    let mut size = std::mem::size_of::<i32>() as u32;
    os(
        AudioObjectGetPropertyData(
            device,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut pid as *mut _ as *mut c_void,
        ),
        "get hog mode",
    )?;
    Ok(pid)
}

fn set_hog_pid(device: AudioObjectID, pid: i32) -> Result<(), MusicError> {
    let addr = prop_addr(kAudioDevicePropertyHogMode);
    let pid = pid;
    os(
        AudioObjectSetPropertyData(
            device,
            &addr,
            0,
            ptr::null(),
            std::mem::size_of::<i32>() as u32,
            &pid as *const _ as *const c_void,
        ),
        "set hog mode",
    )
}

fn output_streams(device: AudioObjectID) -> Result<Vec<AudioStreamID>, MusicError> {
    let addr = prop_addr(kAudioDevicePropertyStreams);
    let mut size: u32 = 0;
    os(
        AudioObjectGetPropertyDataSize(device, &addr, 0, ptr::null(), &mut size),
        "get stream list size",
    )?;
    let count = size as usize / std::mem::size_of::<AudioStreamID>();
    let mut ids: Vec<AudioStreamID> = (0..count).map(|_| 0).collect();
    let mut size2 = size;
    os(
        AudioObjectGetPropertyData(
            device,
            &addr,
            0,
            ptr::null(),
            &mut size2,
            ids.as_mut_ptr() as *mut c_void,
        ),
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
    }
}

fn set_stream_format(
    stream: AudioStreamID,
    asbd: &AudioStreamBasicDescription,
) -> Result<(), MusicError> {
    let addr = prop_addr(kAudioStreamPropertyPhysicalFormat);
    os(
        AudioObjectSetPropertyData(
            stream,
            &addr,
            0,
            ptr::null(),
            std::mem::size_of::<AudioStreamBasicDescription>() as u32,
            asbd as *const _ as *const c_void,
        ),
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
    };
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    os(
        AudioObjectGetPropertyData(
            stream,
            &addr,
            0,
            ptr::null(),
            &mut size,
            &mut asbd as *mut _ as *mut c_void,
        ),
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
    /// Marker for the next underrun-silence frame.
    marker: u8,
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
        // Underrun: synthesize one DoP silence frame. The marker keeps
        // alternating so the DAC never loses frame sync.
        state.underruns.fetch_add(1, Ordering::Relaxed);
        for ch in 0..state.channels as usize {
            dst[ch * 3] = DSD_SILENCE;
            dst[ch * 3 + 1] = DSD_SILENCE;
            dst[ch * 3 + 2] = state.marker;
        }
        state.marker = if state.marker == MARKER_EVEN {
            MARKER_ODD
        } else {
            MARKER_EVEN
        };
    }
}

/// DoP rates the default output device reports as available nominal rates.
pub fn supported_dop_rates() -> Result<Vec<u32>, MusicError> {
    let device = default_output_device()?;
    let avail = available_sample_rates(device)?;
    Ok(DOP_RATES
        .into_iter()
        .filter(|r| avail.iter().any(|a| (*a - *r as f64).abs() < 1.0))
        .collect())
}

pub struct CoreAudioDopSink {
    device: AudioObjectID,
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
        let device = default_output_device()?;
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

impl AudioSink for CoreAudioDopSink {
    fn open(&mut self, track: &Track) -> Result<(), MusicError> {
        self.release();
        let dsd_rate = track
            .sample_rate
            .ok_or_else(|| MusicError::Audio("DoP needs a known DSD rate".into()))?;
        let rate = dop_pcm_rate(dsd_rate)
            .ok_or_else(|| MusicError::Audio(format!("not a DSD rate: {dsd_rate}")))?;
        let channels = track.channels.unwrap_or(2).clamp(1, 8) as u16;

        self.acquire(rate, channels)?;

        let cap = (2 * rate as usize * channels as usize * 3).min(RING_CAP_MAX);
        let (prod, cons) = HeapRb::<u8>::new(cap).split();
        let render_state = Box::new(RenderState {
            cons,
            underruns: self.underruns.clone(),
            channels,
            marker: MARKER_EVEN,
        });
        let raw = Box::into_raw(render_state);
        let mut proc_id: AudioDeviceIOProcID = ptr::null_mut();
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                self.device,
                Some(dop_io_proc),
                raw as *mut c_void,
                &mut proc_id,
            )
        };
        if status != NO_ERR || proc_id.is_null() {
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
        let device = default_output_device().ok()?;
        let avail = available_sample_rates(device).ok()?;
        avail
            .iter()
            .any(|a| (*a - rate as f64).abs() < 1.0)
            .then_some(rate)
    }

    fn select_output_path(&mut self, _path: OutputPath) {
        // This sink *is* the DoP path.
    }

    fn underrun_count(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}
