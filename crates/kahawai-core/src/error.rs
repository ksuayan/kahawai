//! Shared error type. Kept free of HTTP/SQL framework types so
//! `kahawai-player-core` can use it without server dependencies; the server maps
//! these to responses in its API layer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MusicError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Db(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unsatisfiable byte range")]
    BadRange,

    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("metadata error: {0}")]
    Metadata(String),

    #[error("job failed: {0}")]
    JobFailed(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("feature disabled: {feature} ({detail})")]
    FeatureDisabled {
        /// Cargo feature that would enable this, e.g. `encode-opus`.
        feature: String,
        /// Human-readable explanation / remediation.
        detail: String,
    },

    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    #[error("audio device error: {0}")]
    Audio(String),
}
