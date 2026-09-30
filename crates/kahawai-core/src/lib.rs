//! kahawai-core: shared types for the streaming server and the player.
//!
//! Both `kahawai-server` and `kahawai-player-core` depend on this crate. It must stay
//! free of async runtimes, HTTP frameworks, and platform APIs so the player
//! can later target iOS/Android without dragging in server dependencies.

pub mod api;
pub mod config;
pub mod error;
pub mod format;

pub use api::{
    Album, Artist, CatalogDelta, CatalogSnapshot, Genre, ImportPlaylistJson, ImportPlaylistResult,
    Job, JobKind, JobStatus, NewPlaylist, Page, Playlist, PlaylistTracksMode, SetPlaylistTracks,
    StreamFormat, Track,
};
pub use config::{DsdStory, ServerConfig};
pub use error::MusicError;
pub use format::{transcode_ladder, AudioFormat};
