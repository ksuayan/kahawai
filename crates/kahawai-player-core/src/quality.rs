//! One top-level answer to "how should this play?": chase the best sound the
//! output device can give, or stay maximally compatible. The fine controls
//! (stream format, DSD handling, bit-perfect) default to *Auto*, which
//! follows this mode; an explicit value in Settings → Advanced still wins.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityMode {
    /// Exclusive output at each file's own rate on an external DAC, and
    /// native DSD on DACs known to decode it. Yields to the user's own
    /// processing (EQ, loudness, analog stage, software volume) and falls
    /// back to shared output / FLAC whenever the device can't do it.
    #[default]
    Best,
    /// Shared output, DSD converted to FLAC: everything works everywhere,
    /// including EQ and software volume.
    Compatible,
}

/// The user's own processing that exclusive output would bypass, named for
/// the UI ("EQ", "Loudness", "Analog", "Volume").
pub const BLOCKER_EQ: &str = "EQ";
pub const BLOCKER_LOUDNESS: &str = "Loudness";
pub const BLOCKER_ANALOG: &str = "Analog";
pub const BLOCKER_VOLUME: &str = "Volume";
