//! kahawai-player-core: shared playback logic for every platform shell.
//! (Spec: kahawai-player-design.md.)
//!
//! Platform constraint: this crate must compile on any target with **no**
//! audio system libraries and **no** Tauri/platform dependencies. All
//! platform audio (cpal/rodio on desktop, AudioTrack/AVAudioPlayer on mobile)
//! lives behind the [`AudioSink`] trait, implemented by each shell.

pub mod analog;
pub mod artwork;
pub mod bitperfect;
pub mod catalog;
pub mod crossfeed;
pub mod decode;
pub mod dop;
pub mod dsd_devices;
pub mod dsp;
pub mod engine;
pub mod quality;
pub mod queue;
pub mod readahead;
pub mod resample;
pub mod sink;
pub mod transport;

pub use analog::{
    anti_alias_plan, koren_plate_current, oversample_factor, triode_table, AnalogFlavour,
    AnalogSettings, AnalogStage, AnalogStatus, AntiAlias, AntiAliasChoice, TubeTable,
};
pub use artwork::{fetch_from_server as fetch_artwork, ArtworkCache, CachedArt};
pub use bitperfect::{f32_to_i24_le, BitPerfect};
pub use crossfeed::{CrossfeedPreset, CrossfeedSettings, CrossfeedStage};
pub use decode::{DecodedSpec, StreamDecoder};
pub use dop::{dop_pcm_rate, parse_wav_header, DopSpec, DopStream, DOP_BITS_PER_SAMPLE};
pub use dsd_devices::is_known_dsd_device;
pub use dsp::{
    headroom_guard, integrated_lufs, max_boost_db, plan_gain_db, scan_track_levels,
    scan_track_lufs, usable_freq, validate_bands, DspStage, EqBand, EqBandType, GainRamp,
    LookaheadLimiter, LoudnessMeter, LoudnessNorm, ParametricEq, DEFAULT_LOUDNESS_TARGET,
    LIMITER_CEILING, MAX_EQ_BANDS, MAX_LOUDNESS_GAIN_DB, MIN_LOUDNESS_GAIN_DB, NYQUIST_FRACTION,
};
pub use engine::{
    resolve_format, snapshot_key_differs, valid_formats, AnalogLevel, DsdStory, DspSettings,
    EngineCommand, EngineController, Player, PlayerEvent, PlayerSnapshot, PlayerStatus,
    DEFAULT_SERVER_URL,
};
pub use quality::QualityMode;
pub use queue::{Queue, RepeatMode};
pub use readahead::{ReadAhead, ReadAheadStats};
pub use resample::CubicResampler;
pub use sink::{AudioSink, NullSink, OutputPath, PcmChunk, SinkRouter, SinkState, VecSink};
pub use transport::{
    HttpTransport, StreamInfo, StreamOptions, Transport, DEFAULT_READ_AHEAD_BYTES,
};
