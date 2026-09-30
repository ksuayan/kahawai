//! Real audio sinks for the music player.
//!
//! - [`CpalSink`]: shared-mode PCM via cpal (all platforms). The engine's
//!   DSP chain (EQ → loudness → volume) runs ahead of this sink.
//! - `CoreAudioDopSink` (macOS only): exclusive hog-mode DoP output —
//!   bit-perfect 24-bit frames straight to the DAC, bypassing all DSP.
//!
//! `kahawai-player-core` stays platform-independent: it only knows the
//! [`AudioSink`](kahawai_player_core::AudioSink) trait. This crate is where the
//! OS-specific output lives.

mod cpal_sink;

#[cfg(target_os = "macos")]
mod coreaudio;
#[cfg(not(target_os = "macos"))]
mod stub_dop;

pub use cpal_sink::{list_output_devices, CpalSink, DeviceInfo};
pub use kahawai_player_core::{AudioSink, OutputPath, SinkRouter};

#[cfg(target_os = "macos")]
pub use coreaudio::{
    device_capabilities as coreaudio_device_capabilities,
    device_live_state as coreaudio_device_live_state, resolved_device_name, supported_dop_rates,
    CoreAudioDopSink,
};
#[cfg(not(target_os = "macos"))]
pub use stub_dop::StubDopSink;

/// DoP PCM rates the chosen output device (`None` = system default) can
/// take right now.
/// macOS: queried from the device's available nominal rates.
/// Elsewhere: empty (no exclusive DoP path on this OS).
pub fn dop_capable_rates(device: Option<&str>) -> Vec<u32> {
    #[cfg(target_os = "macos")]
    {
        supported_dop_rates(device).unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = device;
        Vec::new()
    }
}

/// What an output device can do, for the Settings screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceCapabilities {
    pub name: String,
    /// "usb" | "thunderbolt" | "firewire" | "built-in" | "bluetooth" |
    /// "hdmi" | "airplay" | "virtual" | "other" | "unknown".
    pub transport: &'static str,
    /// An external DAC-class connection (Best quality may take it exclusively).
    pub external_dac: bool,
    /// Every nominal sample rate the device reports, ascending.
    pub sample_rates: Vec<u32>,
    /// Integer bit depths its stream offers (a 32 usually carries 24 valid bits).
    pub bit_depths: Vec<u32>,
    pub float32: bool,
    /// DoP PCM rates (176400 / 352800 / 705600) it can carry.
    pub dop_rates: Vec<u32>,
    /// The exclusive hog-mode path exists (macOS).
    pub exclusive_available: bool,
}

/// What an output device is doing right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceLiveState {
    pub name: String,
    /// The device's current sample rate.
    pub rate_hz: u32,
    /// Bit depth of its current stream format (0 when unknown).
    pub bit_depth: u32,
    /// The current stream format is floating point (the shared mixer's).
    pub float: bool,
    /// This process holds the device exclusively (hog mode).
    pub exclusive: bool,
}

/// The device's live state (`None` = system default). `None` when
/// unavailable (non-macOS).
pub fn device_live_state(device: Option<&str>) -> Option<DeviceLiveState> {
    #[cfg(target_os = "macos")]
    {
        coreaudio_device_live_state(device)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = device;
        None
    }
}

/// Capabilities of the device output would use (`None` = system default).
/// `None` when unavailable (non-macOS).
pub fn device_capabilities(device: Option<&str>) -> Option<DeviceCapabilities> {
    #[cfg(target_os = "macos")]
    {
        coreaudio_device_capabilities(device)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = device;
        None
    }
}

/// The real name of the device output would use for `device` (`None` = the
/// system default). `None` off macOS when nothing was chosen, or when it
/// cannot be resolved.
pub fn resolved_output_device_name(device: Option<&str>) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        resolved_device_name(device)
    }
    #[cfg(not(target_os = "macos"))]
    {
        device.map(str::to_owned)
    }
}

/// The exclusive DoP side of the [`SinkRouter`]: macOS gets the hog-mode
/// CoreAudio sink, other platforms get a stub that reports no DoP
/// capability (DSD falls back to the PCM path).
pub fn exclusive_dop_sink() -> Box<dyn AudioSink> {
    #[cfg(target_os = "macos")]
    {
        Box::new(CoreAudioDopSink::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(StubDopSink::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn non_macos_has_no_dop_path() {
        assert!(dop_capable_rates(None).is_empty());
        assert!(!exclusive_dop_sink().supports_dop());
    }

    #[test]
    fn cpal_sink_is_dop_free() {
        // The PCM sink must never claim DoP capability: the router uses
        // this to decide which side a DSD track takes.
        assert!(!CpalSink::new().supports_dop());
    }
}
