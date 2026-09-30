//! Shared API types: the JSON contract between server and clients.
//! (Spec §3.3, §3.7.)

use serde::{Deserialize, Serialize};

use crate::format::AudioFormat;

/// One audio file in the catalog. (Spec §3.2 `tracks` table.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub path: String,
    /// BLAKE3 hex digest of the file's contents. `None` until it has been
    /// hashed: the scan catalogs from metadata alone and hashing comes later.
    #[serde(default)]
    pub hash: Option<String>,
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
    /// MQA-encoded file (detected from its `MQAENCODER` tag). Plays as
    /// ordinary FLAC anywhere; an MQA-capable DAC can decode it when the
    /// samples reach it untouched (see the player's bit-perfect output).
    #[serde(default)]
    pub mqa: bool,
    /// Sample rate of the master before MQA folding (`ORIGINALSAMPLERATE`).
    #[serde(default)]
    pub original_sample_rate: Option<u32>,
}

fn default_true() -> bool {
    true
}

/// Album grouping. `track_ids` is populated on detail endpoints; list
/// endpoints may leave it empty to avoid N+1 queries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// Title for sorting ("White Album, The"). Absent from older servers.
    #[serde(default)]
    pub sort_title: Option<String>,
    /// Artist for sorting ("Beatles, The").
    #[serde(default)]
    pub sort_artist: Option<String>,
    /// MusicBrainz release ID, from embedded tags (or later a lookup).
    #[serde(default)]
    pub mbid: Option<String>,
    /// Where the cover came from: "embedded" (or later "caa").
    #[serde(default)]
    pub artwork_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    /// Name for sorting ("Beatles, The"). Absent from older servers.
    #[serde(default)]
    pub sort_name: Option<String>,
}

/// A canonical genre and how many present tracks carry it
/// (`GET /api/genres`). Raw genre tags map to one or more of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genre {
    pub name: String,
    pub track_count: u64,
}

/// `GET /api/identity`: proof that an address is a Kahawai server, and which
/// one. `service` is always [`KAHAWAI_SERVICE`]; `build` names the exact
/// binary; `catalog_id` the library database; `started_at` this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerIdentity {
    pub service: String,
    /// "Kahawai Server".
    pub name: String,
    /// The server's version (Cargo package version).
    pub version: String,
    /// Bumped on breaking API changes, so a client can tell what it can use.
    pub api_version: u32,
    pub build: BuildInfo,
    /// The library database's id (random per database; see the catalog).
    pub catalog_id: String,
    /// When this server process started, Unix ms.
    pub started_at: i64,
}

/// The `service` value of every Kahawai server.
pub const KAHAWAI_SERVICE: &str = "kahawai-server";

/// How a server binary was built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Short git commit, or "unknown".
    pub commit: String,
    /// The source tree had uncommitted changes.
    pub dirty: bool,
    /// UTC, "2026-09-30T19:02:11Z".
    pub built_at: String,
    /// "release" or "debug".
    pub profile: String,
    /// Rust target triple, e.g. "aarch64-apple-darwin".
    pub target: String,
}

/// Everything a player caches (`GET /api/catalog`): present tracks, every
/// album and artist, and the genre list. `rev` is the catalog revision it
/// reflects, `catalog_id` names the server database it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    pub catalog_id: String,
    pub rev: i64,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub genres: Vec<Genre>,
}

/// What changed since a revision (`GET /api/catalog/delta?since=`).
/// Changed rows come whole; a track that went missing comes with
/// `missing: true`. `full_resync` means the delta can't be applied (another
/// database, a revision from the future, or too much changed): pull
/// [`CatalogSnapshot`] instead. `genres` is always the full, current list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogDelta {
    pub catalog_id: String,
    pub rev: i64,
    #[serde(default)]
    pub full_resync: bool,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub removed_tracks: Vec<i64>,
    #[serde(default)]
    pub removed_albums: Vec<i64>,
    #[serde(default)]
    pub removed_artists: Vec<i64>,
    #[serde(default)]
    pub genres: Vec<Genre>,
}

impl CatalogDelta {
    /// Nothing changed (the genre list aside, which always comes whole).
    pub fn is_empty(&self) -> bool {
        !self.full_resync
            && self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.removed_tracks.is_empty()
            && self.removed_albums.is_empty()
            && self.removed_artists.is_empty()
    }
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
    /// When the job started running, in Unix milliseconds (UTC). `None`
    /// while queued, and on servers older than this field.
    #[serde(default)]
    pub started_at: Option<i64>,
    /// When it ended (done, failed, cancelled), in Unix milliseconds. `None`
    /// while queued, running or paused.
    #[serde(default)]
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    ExtractIso,
    Transcode,
    /// Library scan (`POST /api/scan`). Progress is files_processed /
    /// files_total; the result message carries the scan counts (S9).
    Scan,
    /// Content-hash the tracks a scan left pending (`hash IS NULL`). Queued
    /// after every scan; resumes where it left off after a restart.
    HashFiles,
    /// Look up albums without a MusicBrainz ID (MusicBrainz, Cover Art
    /// Archive). Opt-in; can be paused, resumed and cancelled.
    EnrichMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    /// Stopped on request; `resume` carries on where it left off. Survives
    /// a restart (only jobs whose work is resumable can be paused).
    Paused,
    /// Stopped on request for good. Terminal, like Done and Failed.
    Cancelled,
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
            hash: Some("deadbeef".into()),
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
            mqa: false,
            original_sample_rate: None,
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
            sort_title: Some("Kind of Blue".into()),
            sort_artist: Some("Miles Davis".into()),
            mbid: Some("3cc4b4b4-5b0b-4d2d-9d3c-1a9e2f0c4c11".into()),
            artwork_source: Some("embedded".into()),
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
            started_at: Some(1_790_000_000_000),
            finished_at: None,
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

    #[test]
    fn track_json_without_mqa_fields_still_deserializes() {
        // Older servers / saved queues have no mqa fields: they default off.
        let json = r#"{"id":1,"path":"/a.flac","hash":"h","format":"flac","sample_rate":48000,
            "bit_depth":24,"channels":2,"duration_ms":1000,"bitrate":900,"title":"t","album":null,
            "artist":null,"album_id":null,"track_no":null,"disc_no":null}"#;
        let t: Track = serde_json::from_str(json).unwrap();
        assert!(!t.mqa);
        assert_eq!(t.original_sample_rate, None);
        assert!(t.decodable);
    }

    #[test]
    fn a_track_not_yet_hashed_has_no_hash() {
        let json = r#"{"id":1,"path":"/a.flac","hash":null,"format":"flac","sample_rate":null,
            "bit_depth":null,"channels":null,"duration_ms":null,"bitrate":null,"title":"t",
            "album":null,"artist":null,"album_id":null,"track_no":null,"disc_no":null}"#;
        let t: Track = serde_json::from_str(json).unwrap();
        assert_eq!(t.hash, None);
        let saved_queue = r#"{"id":1,"path":"/a.flac","format":"flac","sample_rate":null,
            "bit_depth":null,"channels":null,"duration_ms":null,"bitrate":null,"title":"t",
            "album":null,"artist":null,"album_id":null,"track_no":null,"disc_no":null}"#;
        assert_eq!(
            serde_json::from_str::<Track>(saved_queue).unwrap().hash,
            None
        );
    }

    #[test]
    fn track_mqa_fields_round_trip() {
        let json = r#"{"id":1,"path":"/a.flac","hash":"h","format":"flac","sample_rate":48000,
            "bit_depth":24,"channels":2,"duration_ms":1000,"bitrate":900,"title":"t","album":null,
            "artist":null,"album_id":null,"track_no":null,"disc_no":null,
            "mqa":true,"original_sample_rate":96000}"#;
        let t: Track = serde_json::from_str(json).unwrap();
        assert!(t.mqa);
        assert_eq!(t.original_sample_rate, Some(96_000));
        let back = serde_json::to_string(&t).unwrap();
        assert!(back.contains(r#""mqa":true"#) && back.contains(r#""original_sample_rate":96000"#));
    }
}
