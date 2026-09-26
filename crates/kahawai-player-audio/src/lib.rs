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
pub use coreaudio::{supported_dop_rates, CoreAudioDopSink};
#[cfg(not(target_os = "macos"))]
pub use stub_dop::StubDopSink;

/// DoP PCM rates the default output device can take right now.
/// macOS: queried from the device's available nominal rates.
/// Elsewhere: empty (no exclusive DoP path on this OS).
pub fn dop_capable_rates() -> Vec<u32> {
    #[cfg(target_os = "macos")]
    {
        supported_dop_rates().unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
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
        assert!(dop_capable_rates().is_empty());
        assert!(!exclusive_dop_sink().supports_dop());
    }

    #[test]
    fn cpal_sink_is_dop_free() {
        // The PCM sink must never claim DoP capability: the router uses
        // this to decide which side a DSD track takes.
        assert!(!CpalSink::new().supports_dop());
    }
}
