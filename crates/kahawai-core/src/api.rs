//! Shared API types: the JSON contract between server and clients.
//! (Spec §3.3, §3.7.)

use serde::{Deserialize, Serialize};

use crate::format::AudioFormat;

/// One audio file in the catalog. (Spec §3.2 `tracks` table.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub path: String,
    /// BLAKE3 hex digest — the file's identity for dedupe and change detection.
    pub hash: String,
    pub format: AudioFormat,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub duration_ms: Option<u64>,
    pub bitrate: Option<u32>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub album_id: Option<i64>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    /// Genre tag, if present.
    #[serde(default)]
    pub genre: Option<String>,
    /// Release year, if present.
    #[serde(default)]
    pub year: Option<u16>,
    /// True when the file was missing from disk on the last scan.
    /// Missing tracks stay in the catalog (relink-friendly) but are hidden
    /// from browse listings.
    #[serde(default)]
    pub missing: bool,
    /// False for sources the server cannot stream yet (DSD, SACD ISO —
    /// spec §2, S5). Still cataloged and browsable, not playable.
    #[serde(default = "default_true")]
    pub decodable: bool,
}

fn default_true() -> bool {
    true
}

/// Album grouping. `track_ids` is populated on detail endpoints; list
/// endpoints may leave it empty to avoid N+1 queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<u16>,
    pub artwork_hash: Option<String>,
    #[serde(default)]
    pub track_ids: Vec<i64>,
    /// Number of non-missing tracks on list endpoints. Populated by a
    /// COUNT so listings avoid N+1 queries; 0 on detail endpoints that
    /// carry `track_ids` instead.
    #[serde(default)]
    pub track_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
}

/// Ordered playlist. Positions are 0-based and dense. (Spec §3.2,
/// §3.8.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub track_ids: Vec<i64>,
}

/// Body for `POST /api/playlists`.
/// Body for `POST /api/playlists`. The queue itself lives client-side
/// (spec S11): `from_queue` just marks that `queue_track_ids` carries the
/// client's ordered queue, which the server persists as the playlist's
/// track list. When `from_queue` is false the plain `track_ids` list wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPlaylist {
    pub name: String,
    #[serde(default)]
    pub track_ids: Vec<i64>,
    #[serde(default)]
    pub from_queue: bool,
    #[serde(default)]
    pub queue_track_ids: Vec<i64>,
}

/// How `PUT /api/playlists/:id/tracks` treats the incoming track list
/// (spec S11). Default is `append`: existing entries keep their positions
/// and the new tracks land after them, positions staying dense.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaylistTracksMode {
    #[default]
    Append,
    Replace,
}

/// Body for `PUT /api/playlists/:id/tracks`. Per spec §3.8 the server
/// expands `album_ids` into track IDs in album track order and appends
/// them after `track_ids`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetPlaylistTracks {
    #[serde(default)]
    pub track_ids: Vec<i64>,
    #[serde(default)]
    pub album_ids: Vec<i64>,
    /// Append (default) or replace the existing track list.
    #[serde(default)]
    pub mode: PlaylistTracksMode,
}

/// JSON body for `POST /api/playlists/import` (spec S7 remainder).
/// `path` is a server-local .m3u/.m3u8 file; v1 trusts the LAN client to
/// name it. Multipart upload (a `file` field plus optional `name`) is the
/// alternative input for the same endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportPlaylistJson {
    /// Playlist name; defaults to the file stem when omitted.
    #[serde(default)]
    pub name: Option<String>,
    pub path: String,
}

/// Result of `POST /api/playlists/import`. `unmatched` names the playlist
/// entries that did not resolve to a catalog track, in file order —
/// the client decides how to surface them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportPlaylistResult {
    pub playlist_id: i64,
    pub matched: usize,
    pub unmatched: Vec<String>,
}

/// Long-running server task. (Spec §3.7.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    /// Human-readable label for toasts, e.g. "Extracting SACD ISO: Kind of Blue".
    pub label: String,
    /// Machine-readable job input, e.g. the SACD ISO path for
    /// `extract_iso` jobs. Persisted in the `payload` column (S9).
    #[serde(default)]
    pub payload: Option<String>,
    /// 0.0..=1.0
    pub progress: f32,
    pub status: JobStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    ExtractIso,
    Transcode,
    /// Library scan (`POST /api/scan`). Progress is files_processed /
    /// files_total; the result message carries the scan counts (S9).
    Scan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
}

/// Paginated list envelope for browse endpoints. (Spec §3.3, S2.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// 1-based page number.
    pub page: u64,
    pub per_page: u64,
    /// Total items across all pages.
    pub total: u64,
}

/// Stream rendition selector. (Spec §3.4 "Format options", S13.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamFormat {
    /// Serve the source file bytes untouched. Only valid when the source is
    /// directly streamable (see [`AudioFormat::is_directly_streamable`]).
    #[default]
    Passthrough,
    Flac,
    Opus,
    Mp3,
    /// DoP (DSD over PCM): DSD packed into 24-bit PCM frames in a WAV
    /// container, for DSD-capable DACs. Only valid for DSF/DFF sources
    /// (spec §2, S5b); the DoP rate derives from the source DSD rate.
    Dop,
}

impl StreamFormat {
    /// MIME type of the encoded bytes. `Passthrough` has no single type —
    /// callers serve the source format's MIME instead.
    pub fn mime_type(&self) -> &'static str {
        match self {
            StreamFormat::Passthrough => "application/octet-stream",
            StreamFormat::Flac => "audio/flac",
            StreamFormat::Opus => "audio/ogg",
            StreamFormat::Mp3 => "audio/mpeg",
            // DoP is transported as a WAV (PCM format tag — the standard).
            StreamFormat::Dop => "audio/wav",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::AudioFormat;

    fn sample_track() -> Track {
        Track {
            id: 42,
            path: "/mnt/music/Kind of Blue/01 - So What.flac".into(),
            hash: "deadbeef".into(),
            format: AudioFormat::Flac,
            sample_rate: Some(96000),
            bit_depth: Some(24),
            channels: Some(2),
            duration_ms: Some(545000),
            bitrate: None,
            title: Some("So What".into()),
            album: Some("Kind of Blue".into()),
            artist: Some("Miles Davis".into()),
            album_id: Some(7),
            track_no: Some(1),
            disc_no: Some(1),
            genre: Some("Jazz".into()),
            year: Some(1959),
            missing: false,
            decodable: true,
        }
    }

    fn round_trip<T>(v: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(v).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn track_json_round_trip() {
        let t = sample_track();
        assert_eq!(round_trip(&t), t);
        // Spot-check the wire shape.
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""format":"flac""#));
        assert!(json.contains(r#""title":"So What""#));
    }

    #[test]
    fn track_with_missing_metadata_round_trip() {
        let mut t = sample_track();
        t.title = None;
        t.sample_rate = None;
        t.bitrate = None;
        assert_eq!(round_trip(&t), t);
    }

    #[test]
    fn album_playlist_job_round_trip() {
        let album = Album {
            id: 7,
            title: "Kind of Blue".into(),
            artist: Some("Miles Davis".into()),
            year: Some(1959),
            artwork_hash: Some("abc123".into()),
            track_ids: vec![42, 43, 44],
            track_count: 3,
        };
        assert_eq!(round_trip(&album), album);

        let playlist = Playlist {
            id: 3,
            name: "Late night".into(),
            track_ids: vec![42, 9],
        };
        assert_eq!(round_trip(&playlist), playlist);

        let new = NewPlaylist {
            name: "From queue".into(),
            track_ids: vec![1, 2, 3],
            from_queue: false,
            queue_track_ids: vec![],
        };
        assert_eq!(round_trip(&new), new);

        let job = Job {
            id: "job-0001".into(),
            kind: JobKind::ExtractIso,
            label: "Extracting SACD ISO: Kind of Blue".into(),
            payload: Some("/music/kind-of-blue.iso".into()),
            progress: 0.5,
            status: JobStatus::Running,
            message: None,
        };
        assert_eq!(round_trip(&job), job);
        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains(r#""kind":"extract_iso""#));
        assert!(json.contains(r#""status":"running""#));
    }

    #[test]
    fn stream_format_query_param_shape() {
        // ?format=flac must deserialize (axum Query uses serde).
        let f: StreamFormat = serde_json::from_str(r#""flac""#).unwrap();
        assert_eq!(f, StreamFormat::Flac);
        assert_eq!(StreamFormat::default(), StreamFormat::Passthrough);
    }
}
