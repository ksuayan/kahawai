//! kahawai-player-core: shared playback logic for every platform shell.
//! (Spec: kahawai-player-design.md.)
//!
//! Platform constraint: this crate must compile on any target with **no**
//! audio system libraries and **no** Tauri/platform dependencies. All
//! platform audio (cpal/rodio on desktop, AudioTrack/AVAudioPlayer on mobile)
//! lives behind the [`AudioSink`] trait, implemented by each shell.

pub mod artwork;
pub mod bitperfect;
pub mod decode;
pub mod dop;
pub mod dsp;
pub mod engine;
pub mod queue;
pub mod resample;
pub mod sink;
pub mod transport;

pub use artwork::{fetch_from_server as fetch_artwork, ArtworkCache, CachedArt};
pub use bitperfect::{f32_to_i24_le, BitPerfect};
pub use decode::{DecodedSpec, StreamDecoder};
pub use dop::{dop_pcm_rate, parse_wav_header, DopSpec, DopStream, DOP_BITS_PER_SAMPLE};
pub use dsp::{
    integrated_lufs, scan_track_lufs, validate_bands, EqBand, EqBandType, GainRamp, LoudnessNorm,
    ParametricEq, DEFAULT_LOUDNESS_TARGET, MAX_EQ_BANDS, MAX_LOUDNESS_GAIN_DB,
    MIN_LOUDNESS_GAIN_DB,
};
pub use engine::{
    resolve_format, valid_formats, DsdStory, DspSettings, EngineCommand, EngineController, Player,
    PlayerEvent, PlayerSnapshot, PlayerStatus, DEFAULT_SERVER_URL,
};
pub use queue::{Queue, RepeatMode};
pub use resample::CubicResampler;
pub use sink::{AudioSink, NullSink, OutputPath, PcmChunk, SinkRouter, SinkState, VecSink};
pub use transport::{HttpTransport, StreamInfo, StreamOptions, Transport};
