//! Audio format identification and the transcode ladder. (Spec §2, S13.)

use serde::{Deserialize, Serialize};

use crate::api::StreamFormat;

/// Source audio formats the library can contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Mp3,
    Flac,
    M4a,
    Aac,
    Wav,
    Aiff,
    OggVorbis,
    Opus,
    Dsf,
    Dff,
    SacdIso,
    Unknown,
}

impl AudioFormat {
    /// Identify a format from a file extension (with or without leading dot,
    /// any case). Unknown extensions map to [`AudioFormat::Unknown`] — the
    /// scanner skips those files.
    pub fn from_extension(ext: &str) -> Self {
        match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "mp3" => Self::Mp3,
            "flac" => Self::Flac,
            "m4a" | "mp4" => Self::M4a,
            "aac" => Self::Aac,
            "wav" | "wave" => Self::Wav,
            "aiff" | "aif" | "aifc" => Self::Aiff,
            "ogg" | "oga" => Self::OggVorbis,
            "opus" => Self::Opus,
            "dsf" => Self::Dsf,
            "dff" => Self::Dff,
            // Assumption (spec §2): inside a music library, `.iso` means SACD ISO.
            "iso" => Self::SacdIso,
            _ => Self::Unknown,
        }
    }

    /// Parse the `snake_case` wire/DB name back into a format
    /// (`"ogg_vorbis"`, `"sacd_iso"` — forms [`from_extension`](Self::from_extension)
    /// does not accept). Falls back to extension matching, then [`Unknown`](Self::Unknown).
    pub fn from_wire(s: &str) -> Self {
        match s {
            "ogg_vorbis" => Self::OggVorbis,
            "sacd_iso" => Self::SacdIso,
            other => Self::from_extension(other),
        }
    }

    /// The `snake_case` wire/DB name for this format. Round-trips through
    /// [`from_wire`](Self::from_wire).
    pub fn wire_name(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::M4a => "m4a",
            Self::Aac => "aac",
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::OggVorbis => "ogg_vorbis",
            Self::Opus => "opus",
            Self::Dsf => "dsf",
            Self::Dff => "dff",
            Self::SacdIso => "sacd_iso",
            Self::Unknown => "unknown",
        }
    }
    /// Formats the server can serve byte-for-byte to a capable client without
    /// touching the audio. (Spec §2: symphonia-covered formats, plus Opus via
    /// the `symphonia-adapter-libopus` adapter.)
    pub fn is_directly_streamable(&self) -> bool {
        matches!(
            self,
            Self::Mp3
                | Self::Flac
                | Self::M4a
                | Self::Aac
                | Self::Wav
                | Self::Aiff
                | Self::OggVorbis
                | Self::Opus
        )
    }

    /// MIME type for the `Content-Type` header on passthrough streams.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Flac => "audio/flac",
            Self::M4a => "audio/mp4",
            Self::Aac => "audio/aac",
            Self::Wav => "audio/wav",
            Self::Aiff => "audio/aiff",
            Self::OggVorbis => "audio/ogg",
            // .opus files are Opus audio in an Ogg container.
            Self::Opus => "audio/ogg",
            Self::Dsf | Self::Dff | Self::SacdIso | Self::Unknown => "application/octet-stream",
        }
    }
}

/// Pure function implementing the spec's format ladder (S13).
///
/// - `preference[0]` wins when satisfiable. An explicit non-passthrough
///   request is honored as-is — including lossy→lossy — because it represents
///   the client's deliberate `?format=` override. The "never transcode
///   lossy→lossy" rule is enforced by callers when *choosing* the ladder,
///   not here.
/// - `Passthrough` is only satisfiable for directly streamable sources;
///   DSD and unknown sources fall back to FLAC (Story A: DSD→PCM→FLAC).
/// - Empty preference list: passthrough when possible, else FLAC.
pub fn transcode_ladder(source: AudioFormat, preference: &[StreamFormat]) -> StreamFormat {
    match preference.first() {
        Some(StreamFormat::Passthrough) if source.is_directly_streamable() => {
            StreamFormat::Passthrough
        }
        // DSD / unknown sources cannot be served byte-for-byte.
        Some(StreamFormat::Passthrough) => StreamFormat::Flac,
        Some(other) => *other,
        None if source.is_directly_streamable() => StreamFormat::Passthrough,
        None => StreamFormat::Flac,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_mapping_covers_every_variant() {
        let cases = [
            ("mp3", AudioFormat::Mp3),
            ("flac", AudioFormat::Flac),
            ("m4a", AudioFormat::M4a),
            ("mp4", AudioFormat::M4a),
            ("aac", AudioFormat::Aac),
            ("wav", AudioFormat::Wav),
            ("wave", AudioFormat::Wav),
            ("aiff", AudioFormat::Aiff),
            ("aif", AudioFormat::Aiff),
            ("ogg", AudioFormat::OggVorbis),
            ("oga", AudioFormat::OggVorbis),
            ("opus", AudioFormat::Opus),
            ("dsf", AudioFormat::Dsf),
            ("dff", AudioFormat::Dff),
            ("iso", AudioFormat::SacdIso),
        ];
        for (ext, expected) in cases {
            assert_eq!(AudioFormat::from_extension(ext), expected, "ext={ext}");
        }
    }

    #[test]
    fn extension_mapping_is_case_and_dot_insensitive() {
        assert_eq!(AudioFormat::from_extension("FLAC"), AudioFormat::Flac);
        assert_eq!(AudioFormat::from_extension(".Mp3"), AudioFormat::Mp3);
        assert_eq!(AudioFormat::from_extension(".DFF"), AudioFormat::Dff);
    }

    #[test]
    fn unknown_extensions_map_to_unknown() {
        for ext in ["", "xyz", "exe", "cue", "log", "jpg"] {
            assert_eq!(
                AudioFormat::from_extension(ext),
                AudioFormat::Unknown,
                "ext={ext}"
            );
        }
    }

    #[test]
    fn wire_name_round_trips_through_from_wire() {
        for fmt in [
            AudioFormat::Mp3,
            AudioFormat::Flac,
            AudioFormat::M4a,
            AudioFormat::Aac,
            AudioFormat::Wav,
            AudioFormat::Aiff,
            AudioFormat::OggVorbis,
            AudioFormat::Opus,
            AudioFormat::Dsf,
            AudioFormat::Dff,
            AudioFormat::SacdIso,
            AudioFormat::Unknown,
        ] {
            assert_eq!(AudioFormat::from_wire(fmt.wire_name()), fmt, "{fmt:?}");
        }
        // DB rows written with wire names must not read back as Unknown.
        assert_eq!(AudioFormat::from_wire("ogg_vorbis"), AudioFormat::OggVorbis);
        assert_eq!(AudioFormat::from_wire("sacd_iso"), AudioFormat::SacdIso);
    }

    #[test]
    fn directly_streamable_set_matches_spec() {
        for fmt in [
            AudioFormat::Mp3,
            AudioFormat::Flac,
            AudioFormat::M4a,
            AudioFormat::Aac,
            AudioFormat::Wav,
            AudioFormat::Aiff,
            AudioFormat::OggVorbis,
            AudioFormat::Opus,
        ] {
            assert!(fmt.is_directly_streamable(), "{fmt:?}");
        }
        for fmt in [
            AudioFormat::Dsf,
            AudioFormat::Dff,
            AudioFormat::SacdIso,
            AudioFormat::Unknown,
        ] {
            assert!(!fmt.is_directly_streamable(), "{fmt:?}");
        }
    }

    #[test]
    fn ladder_prefers_passthrough_for_streamable_sources() {
        for fmt in [
            AudioFormat::Mp3,
            AudioFormat::Flac,
            AudioFormat::M4a,
            AudioFormat::Aac,
            AudioFormat::OggVorbis,
            AudioFormat::Opus,
            AudioFormat::Wav,
            AudioFormat::Aiff,
        ] {
            assert_eq!(transcode_ladder(fmt, &[]), StreamFormat::Passthrough);
            assert_eq!(
                transcode_ladder(fmt, &[StreamFormat::Passthrough]),
                StreamFormat::Passthrough
            );
        }
    }

    #[test]
    fn ladder_falls_back_to_flac_for_dsd_sources() {
        for fmt in [AudioFormat::Dsf, AudioFormat::Dff, AudioFormat::SacdIso] {
            assert_eq!(transcode_ladder(fmt, &[]), StreamFormat::Flac);
            // Passthrough requested but not satisfiable -> FLAC (Story A).
            assert_eq!(
                transcode_ladder(fmt, &[StreamFormat::Passthrough]),
                StreamFormat::Flac
            );
            // Explicit transcode target honored.
            assert_eq!(
                transcode_ladder(fmt, &[StreamFormat::Opus]),
                StreamFormat::Opus
            );
        }
        // Unknown sources behave like DSD: never passthrough.
        assert_eq!(
            transcode_ladder(AudioFormat::Unknown, &[StreamFormat::Passthrough]),
            StreamFormat::Flac
        );
    }

    #[test]
    fn ladder_honors_explicit_format_override() {
        // Deliberate ?format= choice, including lossy->lossy: honored here;
        // the "never transcode lossy→lossy" policy lives with the caller.
        assert_eq!(
            transcode_ladder(AudioFormat::Mp3, &[StreamFormat::Mp3]),
            StreamFormat::Mp3
        );
        assert_eq!(
            transcode_ladder(AudioFormat::Flac, &[StreamFormat::Opus]),
            StreamFormat::Opus
        );
        assert_eq!(
            transcode_ladder(AudioFormat::M4a, &[StreamFormat::Flac]),
            StreamFormat::Flac
        );
    }
}
