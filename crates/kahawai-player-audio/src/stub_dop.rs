//! Stub exclusive-DoP sink for non-macOS targets.
//!
//! There is no hog-mode/DoP path outside macOS: this sink reports no DoP
//! capability, so the engine's capability check routes DSD through the
//! PCM fallback (DSD→PCM/FLAC) before this sink ever sees a byte. It exists
//! so the Tauri shell can build the same [`SinkRouter`] on every OS.

use kahawai_core::{MusicError, Track};
use kahawai_player_core::{AudioSink, OutputPath, PcmChunk, SinkState};

pub struct StubDopSink {
    state: SinkState,
}

impl StubDopSink {
    pub fn new() -> Self {
        Self {
            state: SinkState::Stopped,
        }
    }
}

impl Default for StubDopSink {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSink for StubDopSink {
    fn open(&mut self, _track: &Track) -> Result<(), MusicError> {
        Err(MusicError::Audio(
            "exclusive DoP output is macOS-only".into(),
        ))
    }

    fn write(&mut self, _chunk: PcmChunk) -> Result<(), MusicError> {
        Err(MusicError::Audio("stub DoP sink accepts nothing".into()))
    }

    fn play(&mut self) -> Result<(), MusicError> {
        Ok(())
    }

    fn pause(&mut self) -> Result<(), MusicError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), MusicError> {
        self.state = SinkState::Stopped;
        Ok(())
    }

    fn state(&self) -> SinkState {
        self.state
    }

    fn supports_dop(&self) -> bool {
        false
    }

    fn select_output_path(&mut self, _path: OutputPath) {}
}
