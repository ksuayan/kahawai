//! Playlist exports for third-party players such as VLC: M3U, XSPF and
//! PLS renderings of a playlist or album, one absolute `/stream` URL per
//! track. (docs/v1/kahawai-server-vlc-client-spec.md, D1.)
//!
//! Exports never use `?next=`: that is the first-party player's gapless
//! contract, and VLC advances its own playlist. Each entry names a rendition
//! VLC can always play, and entries the server could not serve (SACD ISO,
//! files gone from disk) are left out and counted in `X-Export-Skipped`.

use std::collections::HashMap;
use std::fmt::Write as _;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, Uri},
    response::Response,
};
use kahawai_core::{AudioFormat, MusicError, StreamFormat, Track};
use serde::Deserialize;
use sqlx::Row;

use crate::{api::ApiError, db, stream, AppState};

/// Response header counting the tracks left out of an export.
pub(crate) const EXPORT_SKIPPED_HEADER: &str = "x-export-skipped";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// Plain `.m3u`, not `.m3u8`: VLC sniffs `.m3u8` for HLS tags.
    #[default]
    M3u,
    Xspf,
    Pls,
}

impl ExportFormat {
    fn content_type(self) -> &'static str {
        match self {
            Self::M3u => "audio/x-mpegurl; charset=utf-8",
            Self::Xspf => "application/xspf+xml; charset=utf-8",
            Self::Pls => "audio/x-scpls; charset=utf-8",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::Xspf => "xspf",
            Self::Pls => "pls",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    pub format: ExportFormat,
}

/// The rendition an export asks for, or `None` when the track must be left
/// out. Passthrough for everything VLC decodes natively; FLAC for DSD (never
/// DSD bytes, never DoP) and for unknown sources (never an
/// `application/octet-stream` passthrough). SACD ISO has no decode path on
/// the server (it would be a 415).
pub fn vlc_rendition(format: AudioFormat) -> Option<StreamFormat> {
    match format {
        AudioFormat::SacdIso => None,
        f if f.is_directly_streamable() => Some(StreamFormat::Passthrough),
        _ => Some(StreamFormat::Flac),
    }
}

fn rendition_wire(f: StreamFormat) -> &'static str {
    match f {
        StreamFormat::Passthrough => "passthrough",
        StreamFormat::Flac => "flac",
        StreamFormat::Opus => "opus",
        StreamFormat::Mp3 => "mp3",
        StreamFormat::Dop => "dop",
    }
}

/// One playable export entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub location: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub image: Option<String>,
}

impl Entry {
    /// `Artist - Title`, or the title alone.
    fn display(&self) -> String {
        match &self.artist {
            Some(a) => format!("{a} - {}", self.title),
            None => self.title.clone(),
        }
    }

    /// Whole seconds for M3U/PLS: rounded, at least 1 for any known
    /// duration, `-1` when unknown. Never 0 — players read that as "empty".
    fn duration_s(&self) -> i64 {
        match self.duration_ms {
            Some(ms) if ms > 0 => ((ms + 500) / 1000).max(1) as i64,
            _ => -1,
        }
    }
}

/// One line of text: line breaks and other control characters would
/// split an M3U/PLS entry, so they become spaces.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Non-empty, single-line tag value.
fn tag(s: &Option<String>) -> Option<String> {
    s.as_deref().map(one_line).filter(|s| !s.is_empty())
}

pub fn render_m3u(entries: &[Entry]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for e in entries {
        let _ = writeln!(out, "#EXTINF:{},{}", e.duration_s(), e.display());
        let _ = writeln!(out, "{}", e.location);
    }
    out
}

pub fn render_pls(entries: &[Entry]) -> String {
    let mut out = String::from("[playlist]\n");
    for (i, e) in entries.iter().enumerate() {
        let n = i + 1;
        let _ = writeln!(out, "File{n}={}", e.location);
        let _ = writeln!(out, "Title{n}={}", e.display());
        let _ = writeln!(out, "Length{n}={}", e.duration_s());
    }
    let _ = writeln!(out, "NumberOfEntries={}", entries.len());
    out.push_str("Version=2\n");
    out
}

/// XML text escaping; characters XML 1.0 cannot carry at all are dropped.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

pub fn render_xspf(title: &str, entries: &[Entry]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\">\n",
    );
    let _ = writeln!(out, "  <title>{}</title>", xml_escape(title));
    out.push_str("  <trackList>\n");
    for e in entries {
        out.push_str("    <track>\n");
        let _ = writeln!(
            out,
            "      <location>{}</location>",
            xml_escape(&e.location)
        );
        let _ = writeln!(out, "      <title>{}</title>", xml_escape(&e.title));
        if let Some(a) = &e.artist {
            let _ = writeln!(out, "      <creator>{}</creator>", xml_escape(a));
        }
        if let Some(a) = &e.album {
            let _ = writeln!(out, "      <album>{}</album>", xml_escape(a));
        }
        if let Some(ms) = e.duration_ms.filter(|&ms| ms > 0) {
            let _ = writeln!(out, "      <duration>{ms}</duration>");
        }
        if let Some(img) = &e.image {
            let _ = writeln!(out, "      <image>{}</image>", xml_escape(img));
        }
        out.push_str("    </track>\n");
    }
    out.push_str("  </trackList>\n</playlist>\n");
    out
}

/// `http://{host}` for the request, from its `Host` header (or the URI
/// authority, as HTTP/2 sends it). The port comes with it, so the URLs
/// point wherever the client reached this server. Validated before it is
/// written into a playlist: a host name, IPv4 or bracketed IPv6 address
/// and an optional port, nothing else.
pub fn base_url(headers: &HeaderMap, uri: &Uri) -> Result<String, MusicError> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| uri.authority().map(|a| a.to_string()))
        .ok_or_else(|| MusicError::BadRequest("a Host header is required".into()))?;
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'));
    if !valid {
        return Err(MusicError::BadRequest("invalid Host header".into()));
    }
    Ok(format!("http://{host}"))
}

/// The catalog fields an export needs, beyond the [`Track`] itself.
struct Candidate {
    track: Track,
    image_hash: Option<String>,
}

/// Turn catalog tracks into entries, leaving out (and counting) the ones
/// that cannot play: SACD ISO, flagged missing, or not on disk under a
/// music root right now.
fn build_entries(
    s: &AppState,
    base: &str,
    candidates: Vec<Candidate>,
    unknown_ids: usize,
) -> (Vec<Entry>, usize) {
    let roots = s.music_dirs();
    let mut skipped = unknown_ids;
    let mut entries = Vec::with_capacity(candidates.len());
    for Candidate { track, image_hash } in candidates {
        let Some(rendition) = vlc_rendition(track.format) else {
            skipped += 1;
            continue;
        };
        let path = std::path::Path::new(&track.path);
        if track.missing || crate::api::ensure_within_roots(path, &roots).is_err() {
            skipped += 1;
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("track-{}", track.id));
        entries.push(Entry {
            location: format!(
                "{base}/stream/{}?format={}",
                track.id,
                rendition_wire(rendition)
            ),
            title: tag(&track.title).unwrap_or_else(|| one_line(&stem)),
            artist: tag(&track.artist),
            album: tag(&track.album),
            duration_ms: track.duration_ms,
            image: image_hash.map(|h| format!("{base}/api/artwork/{h}")),
        });
    }
    (entries, skipped)
}

/// Artwork per track, the track's own first, else its album's — in one
/// query for the whole export.
async fn artwork_for(
    s: &AppState,
    clause: &str,
    arg: i64,
) -> Result<HashMap<i64, String>, MusicError> {
    let rows = sqlx::query(&format!(
        "SELECT t.id, COALESCE(t.artwork_hash, a.artwork_hash) AS art \
         FROM tracks t LEFT JOIN albums a ON a.id = t.album_id WHERE {clause}"
    ))
    .bind(arg)
    .fetch_all(&s.pool)
    .await
    .map_err(db::cvt)?;
    Ok(rows
        .iter()
        .filter_map(|r| Some((r.get("id"), r.get::<Option<String>, _>("art")?)))
        .collect())
}

fn respond(
    format: ExportFormat,
    name: &str,
    fallback_stem: &str,
    entries: &[Entry],
    skipped: usize,
) -> Result<Response, ApiError> {
    let body = match format {
        ExportFormat::M3u => render_m3u(entries),
        ExportFormat::Xspf => render_xspf(name, entries),
        ExportFormat::Pls => render_pls(entries),
    };
    let ext = format.extension();
    Response::builder()
        .header(header::CONTENT_TYPE, format.content_type())
        .header(
            header::CONTENT_DISPOSITION,
            stream::content_disposition(
                "attachment",
                &format!("{name}.{ext}"),
                &format!("{fallback_stem}.{ext}"),
            ),
        )
        .header(EXPORT_SKIPPED_HEADER, skipped)
        .body(axum::body::Body::from(body))
        .map_err(|e| ApiError::from(MusicError::Http(e.to_string())))
}

/// `GET /api/playlists/{id}/export?format=m3u|xspf|pls`, in playlist order.
pub async fn export_playlist(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<ExportQuery>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, ApiError> {
    let base = base_url(&headers, &uri)?;
    let name: String = sqlx::query("SELECT name FROM playlists WHERE id = ?")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(db::cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("playlist {id}")))?
        .get("name");
    let order: Vec<i64> =
        sqlx::query("SELECT track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position")
            .bind(id)
            .fetch_all(&s.pool)
            .await
            .map_err(db::cvt)?
            .iter()
            .map(|r| r.get("track_id"))
            .collect();

    const IN_PLAYLIST: &str = "id IN (SELECT track_id FROM playlist_tracks WHERE playlist_id = ?)";
    const T_IN_PLAYLIST: &str =
        "t.id IN (SELECT track_id FROM playlist_tracks WHERE playlist_id = ?)";
    let mut conn = s.pool.acquire().await.map_err(db::cvt)?;
    let tracks: HashMap<i64, Track> = db::tracks_where(&mut conn, IN_PLAYLIST, id)
        .await?
        .into_iter()
        .map(|t| (t.id, t))
        .collect();
    drop(conn);
    let art = artwork_for(&s, T_IN_PLAYLIST, id).await?;

    // A playlist may name the same track twice; it plays twice.
    let mut candidates = Vec::with_capacity(order.len());
    let mut unknown = 0;
    for tid in order {
        match tracks.get(&tid) {
            Some(t) => candidates.push(Candidate {
                track: t.clone(),
                image_hash: art.get(&tid).cloned(),
            }),
            None => unknown += 1,
        }
    }
    let (entries, skipped) = build_entries(&s, &base, candidates, unknown);
    respond(
        q.format,
        &name,
        &format!("playlist-{id}"),
        &entries,
        skipped,
    )
}

/// `GET /api/albums/{id}/export?format=m3u|xspf|pls`, in disc/track order.
/// Tracks flagged missing count as skipped, like a playlist's.
pub async fn export_album(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<ExportQuery>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, ApiError> {
    let base = base_url(&headers, &uri)?;
    let row = sqlx::query("SELECT title, artist FROM albums WHERE id = ?")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(db::cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("album {id}")))?;
    let title: String = row.get("title");
    let name = match row.get::<Option<String>, _>("artist") {
        Some(artist) if !artist.trim().is_empty() => format!("{artist} - {title}"),
        _ => title,
    };

    const IN_ALBUM: &str = "album_id = ? AND duplicate_of IS NULL";
    const T_IN_ALBUM: &str = "t.album_id = ? AND t.duplicate_of IS NULL";
    let mut conn = s.pool.acquire().await.map_err(db::cvt)?;
    let mut tracks = db::tracks_where(&mut conn, IN_ALBUM, id).await?;
    drop(conn);
    // Same order as the album view (`db::tracks_for_album`): unnumbered
    // discs and tracks first, then by id.
    tracks.sort_by_key(|t| (t.disc_no, t.track_no, t.id));
    let art = artwork_for(&s, T_IN_ALBUM, id).await?;

    let candidates = tracks
        .into_iter()
        .map(|t| Candidate {
            image_hash: art.get(&t.id).cloned(),
            track: t,
        })
        .collect();
    let (entries, skipped) = build_entries(&s, &base, candidates, 0);
    respond(q.format, &name, &format!("album-{id}"), &entries, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration_tests::{dsf_fixture, wav_fixture};
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        Router,
    };
    use kahawai_core::config::ServerConfig;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn entry(title: &str, artist: Option<&str>, ms: Option<u64>) -> Entry {
        Entry {
            location: "http://h:8080/stream/1?format=passthrough".into(),
            title: title.into(),
            artist: artist.map(Into::into),
            album: None,
            duration_ms: ms,
            image: None,
        }
    }

    #[test]
    fn rendition_table_matches_the_spec() {
        for f in [
            AudioFormat::Mp3,
            AudioFormat::Flac,
            AudioFormat::M4a,
            AudioFormat::Aac,
            AudioFormat::Wav,
            AudioFormat::Aiff,
            AudioFormat::OggVorbis,
            AudioFormat::Opus,
        ] {
            assert_eq!(vlc_rendition(f), Some(StreamFormat::Passthrough), "{f:?}");
        }
        for f in [AudioFormat::Dsf, AudioFormat::Dff, AudioFormat::Unknown] {
            assert_eq!(vlc_rendition(f), Some(StreamFormat::Flac), "{f:?}");
        }
        assert_eq!(vlc_rendition(AudioFormat::SacdIso), None);
    }

    #[test]
    fn durations_round_and_are_never_zero() {
        assert_eq!(entry("t", None, Some(185_400)).duration_s(), 185);
        assert_eq!(entry("t", None, Some(185_500)).duration_s(), 186);
        assert_eq!(entry("t", None, Some(300)).duration_s(), 1);
        assert_eq!(entry("t", None, Some(0)).duration_s(), -1);
        assert_eq!(entry("t", None, None).duration_s(), -1);
    }

    #[test]
    fn m3u_rendering() {
        let m3u = render_m3u(&[
            entry("So What", Some("Miles Davis"), Some(562_000)),
            entry("untitled", None, None),
        ]);
        assert_eq!(
            m3u,
            "#EXTM3U\n\
             #EXTINF:562,Miles Davis - So What\n\
             http://h:8080/stream/1?format=passthrough\n\
             #EXTINF:-1,untitled\n\
             http://h:8080/stream/1?format=passthrough\n"
        );
    }

    #[test]
    fn pls_rendering() {
        let pls = render_pls(&[entry("A", Some("B"), Some(1_000)), entry("C", None, None)]);
        assert_eq!(
            pls,
            "[playlist]\n\
             File1=http://h:8080/stream/1?format=passthrough\n\
             Title1=B - A\n\
             Length1=1\n\
             File2=http://h:8080/stream/1?format=passthrough\n\
             Title2=C\n\
             Length2=-1\n\
             NumberOfEntries=2\n\
             Version=2\n"
        );
    }

    #[test]
    fn xspf_escapes_text_and_drops_unknown_fields() {
        let mut e = entry("Rock & <Roll>", Some("\"Q\" 'R'\u{1}"), None);
        e.album = Some("Alb".into());
        e.image = Some("http://h:8080/api/artwork/ab".into());
        let x = render_xspf("Mix & Match", &[e]);
        assert!(x.contains("<title>Mix &amp; Match</title>"), "{x}");
        assert!(x.contains("<title>Rock &amp; &lt;Roll&gt;</title>"), "{x}");
        assert!(
            x.contains("<creator>&quot;Q&quot; &apos;R&apos;</creator>"),
            "{x}"
        );
        assert!(x.contains("<album>Alb</album>"), "{x}");
        assert!(
            x.contains("<image>http://h:8080/api/artwork/ab</image>"),
            "{x}"
        );
        assert!(!x.contains("<duration>"), "unknown duration omitted: {x}");
    }

    #[test]
    fn tag_values_are_single_line() {
        assert_eq!(tag(&Some("a\r\nb\tc ".into())), Some("a  b c".into()));
        assert_eq!(tag(&Some(" \n ".into())), None);
    }

    #[test]
    fn base_url_uses_the_host_header_and_validates_it() {
        let uri: Uri = "/api/playlists/1/export".parse().unwrap();
        let mut h = HeaderMap::new();
        for ok in [
            "192.168.1.20:8080",
            "nas.local:8080",
            "[fe80::1]:8080",
            "music",
        ] {
            h.insert(header::HOST, ok.parse().unwrap());
            assert_eq!(base_url(&h, &uri).unwrap(), format!("http://{ok}"));
        }
        for bad in ["evil.com/x", "a b", "h\"x", "h<x>"] {
            h.insert(header::HOST, bad.parse().unwrap());
            assert!(base_url(&h, &uri).is_err(), "{bad}");
        }
        // HTTP/2: no Host header, the authority is in the URI.
        let uri: Uri = "http://10.0.0.5:8080/api/albums/1/export".parse().unwrap();
        assert_eq!(
            base_url(&HeaderMap::new(), &uri).unwrap(),
            "http://10.0.0.5:8080"
        );
        assert!(base_url(&HeaderMap::new(), &"/x".parse().unwrap()).is_err());
    }

    // ------------------------------------------------------------------
    // Through the router
    // ------------------------------------------------------------------

    struct Lib {
        app: Router,
        state: AppState,
        _dir: tempfile::TempDir,
    }

    async fn track(
        pool: &sqlx::SqlitePool,
        dir: &std::path::Path,
        file: &str,
        format: &str,
        bytes: &[u8],
    ) -> i64 {
        let path = dir.join(file);
        std::fs::write(&path, bytes).unwrap();
        db::insert_track_minimal(pool, path.to_str().unwrap(), "h", format)
            .await
            .unwrap()
    }

    async fn tag_track(
        pool: &sqlx::SqlitePool,
        id: i64,
        title: Option<&str>,
        artist: Option<&str>,
        album_id: Option<i64>,
        disc_track: (Option<i64>, Option<i64>),
        duration_ms: Option<i64>,
    ) {
        sqlx::query(
            "UPDATE tracks SET title = ?, artist = ?, album = 'Kind of Blue', album_id = ?, \
             disc_no = ?, track_no = ?, duration_ms = ? WHERE id = ?",
        )
        .bind(title)
        .bind(artist)
        .bind(album_id)
        .bind(disc_track.0)
        .bind(disc_track.1)
        .bind(duration_ms)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// A library with one of everything the rendition table cares about.
    /// Track ids: 1 mp3, 2 flac, 3 dsf, 4 sacd iso, 5 mp3 deleted from disk,
    /// 6 untagged wav, 7 CJK-titled flac flagged missing by a scan.
    /// Album 1 holds all of them; playlist 1 lists them out of id order.
    async fn library() -> Lib {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("music");
        std::fs::create_dir(&root).unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();

        let album: i64 = sqlx::query(
            "INSERT INTO albums (title, artist, artwork_hash) \
             VALUES ('Kind of Blue', 'Miles Davis', 'aa11') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("id");
        let ids = [
            track(&pool, &root, "01 So What.mp3", "mp3", b"mp3").await,
            track(&pool, &root, "02 Freddie.flac", "flac", b"flac").await,
            track(
                &pool,
                &root,
                "03 Blue.dsf",
                "dsf",
                &dsf_fixture(2, 4096, 0x69),
            )
            .await,
            track(&pool, &root, "04 Disc.iso", "sacd_iso", b"iso").await,
            track(&pool, &root, "05 Gone.mp3", "mp3", b"gone").await,
            track(
                &pool,
                &root,
                "06 untagged.wav",
                "wav",
                &wav_fixture(8000, 1, 80),
            )
            .await,
            track(&pool, &root, "07 cjk.flac", "flac", b"cjk").await,
        ];
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7]);
        std::fs::remove_file(root.join("05 Gone.mp3")).unwrap();
        // (title, artist, (disc, track), duration_ms)
        type Tags<'a> = (Option<&'a str>, Option<&'a str>, (i64, i64), Option<i64>);
        let tags: [Tags; 7] = [
            (Some("So What"), Some("Miles Davis"), (1, 1), Some(562_400)),
            (
                Some("Freddie Freeloader"),
                Some("Miles Davis"),
                (1, 2),
                Some(589_000),
            ),
            (
                Some("Blue in Green"),
                Some("Miles Davis"),
                (1, 3),
                Some(337_600),
            ),
            (
                Some("All Blues"),
                Some("Miles Davis"),
                (1, 4),
                Some(693_000),
            ),
            (
                Some("Flamenco Sketches"),
                Some("Miles Davis"),
                (1, 5),
                Some(565_000),
            ),
            (None, None, (2, 1), None),
            (
                Some("戦場のメリークリスマス"),
                Some("坂本龍一"),
                (2, 2),
                Some(1),
            ),
        ];
        for (id, (title, artist, (d, t), ms)) in ids.iter().zip(tags) {
            tag_track(
                &pool,
                *id,
                title,
                artist,
                Some(album),
                (Some(d), Some(t)),
                ms,
            )
            .await;
        }
        // Track 1 has its own art; the rest inherit the album's.
        sqlx::query("UPDATE tracks SET artwork_hash = 'bb22' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tracks SET missing = 1 WHERE id = 7")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("INSERT INTO playlists (id, name) VALUES (1, 'Late Night: Blue')")
            .execute(&pool)
            .await
            .unwrap();
        for (pos, tid) in [3, 1, 4, 5, 2, 6].iter().enumerate() {
            sqlx::query(
                "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (1, ?, ?)",
            )
            .bind(pos as i64)
            .bind(tid)
            .execute(&pool)
            .await
            .unwrap();
        }

        let state = AppState {
            pool,
            jobs: crate::jobs::JobStore::new(),
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![root],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: crate::transcode_cache::TranscodeCache::disabled(),
        };
        Lib {
            app: crate::app(state.clone()),
            state,
            _dir: dir,
        }
    }

    async fn get(app: &Router, uri: &str, host: &str) -> (StatusCode, HeaderMap, String) {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::HOST, host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    /// The spec's acceptance check: mixed formats incl. a DSF, a SACD ISO
    /// and a missing file.
    #[tokio::test]
    async fn playlist_m3u_export_acceptance() {
        let lib = library().await;
        let (status, h, body) = get(
            &lib.app,
            "/api/playlists/1/export?format=m3u",
            "192.168.1.20:8080",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(h[header::CONTENT_TYPE], "audio/x-mpegurl; charset=utf-8");
        assert_eq!(
            h[header::CONTENT_DISPOSITION],
            "attachment; filename=\"Late Night: Blue.m3u\""
        );
        // The SACD ISO and the file gone from disk.
        assert_eq!(h[EXPORT_SKIPPED_HEADER], "2");
        assert_eq!(
            body,
            "#EXTM3U\n\
             #EXTINF:338,Miles Davis - Blue in Green\n\
             http://192.168.1.20:8080/stream/3?format=flac\n\
             #EXTINF:562,Miles Davis - So What\n\
             http://192.168.1.20:8080/stream/1?format=passthrough\n\
             #EXTINF:589,Miles Davis - Freddie Freeloader\n\
             http://192.168.1.20:8080/stream/2?format=passthrough\n\
             #EXTINF:-1,06 untagged\n\
             http://192.168.1.20:8080/stream/6?format=passthrough\n"
        );
        assert!(!body.contains("next=") && !body.contains("format=dop"));
    }

    /// Every URL an export emits plays (no 404, no 415) — including the
    /// DSD entry, which the server transcodes to FLAC.
    #[tokio::test]
    async fn every_exported_url_plays() {
        let lib = library().await;
        let (_, _, body) = get(&lib.app, "/api/playlists/1/export", "kahawai:8080").await;
        let urls: Vec<&str> = body.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(urls.len(), 4, "{body}");
        for url in urls {
            let path = url.strip_prefix("http://kahawai:8080").expect(url);
            let res = lib
                .app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{url}");
            let ct = res.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .to_string();
            assert_ne!(ct, "application/octet-stream", "{url}");
        }
    }

    #[tokio::test]
    async fn playlist_xspf_and_pls_exports() {
        let lib = library().await;
        let (status, h, x) = get(&lib.app, "/api/playlists/1/export?format=xspf", "nas:8080").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            h[header::CONTENT_TYPE],
            "application/xspf+xml; charset=utf-8"
        );
        assert_eq!(h[EXPORT_SKIPPED_HEADER], "2");
        assert_eq!(x.matches("<track>").count(), 4, "{x}");
        assert!(x.contains("<title>Late Night: Blue</title>"), "{x}");
        assert!(x.contains("<location>http://nas:8080/stream/3?format=flac</location>"));
        assert!(x.contains("<duration>337600</duration>"), "{x}");
        assert!(x.contains("<album>Kind of Blue</album>"), "{x}");
        // Track 1's own art wins over the album's; the rest inherit it.
        assert!(
            x.contains("<image>http://nas:8080/api/artwork/bb22</image>"),
            "{x}"
        );
        assert_eq!(x.matches("/api/artwork/aa11").count(), 3, "{x}");

        let (status, h, pls) =
            get(&lib.app, "/api/playlists/1/export?format=pls", "nas:8080").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "audio/x-scpls; charset=utf-8");
        assert!(pls.starts_with("[playlist]\nFile1=http://nas:8080/stream/3?format=flac\n"));
        assert!(
            pls.contains("Title1=Miles Davis - Blue in Green\nLength1=338\n"),
            "{pls}"
        );
        assert!(pls.ends_with("NumberOfEntries=4\nVersion=2\n"), "{pls}");
    }

    #[tokio::test]
    async fn album_export_is_in_disc_track_order() {
        let lib = library().await;
        // Shuffle the stored track numbers so disc/track order differs from id order.
        sqlx::query("UPDATE tracks SET track_no = 9 WHERE id = 1")
            .execute(&lib.state.pool)
            .await
            .unwrap();
        let (status, h, body) = get(&lib.app, "/api/albums/1/export", "nas:8080").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            h[header::CONTENT_DISPOSITION],
            "attachment; filename=\"Miles Davis - Kind of Blue.m3u\""
        );
        // SACD ISO, file gone, and the CJK track flagged missing.
        assert_eq!(h[EXPORT_SKIPPED_HEADER], "3");
        let ids: Vec<&str> = body
            .lines()
            .filter_map(|l| l.split("/stream/").nth(1))
            .map(|rest| rest.split('?').next().unwrap())
            .collect();
        assert_eq!(ids, ["2", "3", "1", "6"], "{body}");
    }

    #[tokio::test]
    async fn non_ascii_names_use_rfc5987() {
        let lib = library().await;
        sqlx::query("UPDATE playlists SET name = '深夜 Jazz' WHERE id = 1")
            .execute(&lib.state.pool)
            .await
            .unwrap();
        let (_, h, _) = get(&lib.app, "/api/playlists/1/export?format=pls", "nas:8080").await;
        assert_eq!(
            h[header::CONTENT_DISPOSITION],
            "attachment; filename=\"playlist-1.pls\"; \
             filename*=UTF-8''%E6%B7%B1%E5%A4%9C%20Jazz.pls"
        );
    }

    #[tokio::test]
    async fn export_errors() {
        let lib = library().await;
        let (s, _, _) = get(&lib.app, "/api/playlists/42/export", "nas:8080").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _, _) = get(&lib.app, "/api/albums/42/export", "nas:8080").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _, _) = get(&lib.app, "/api/playlists/1/export?format=m3u8", "nas:8080").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _, _) = get(&lib.app, "/api/playlists/1/export", "nas:8080/#x").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    /// Test plan item 5: a 500-track playlist exports in well under a second.
    #[tokio::test]
    async fn large_playlist_exports_quickly() {
        let lib = library().await;
        for pos in 6..506 {
            sqlx::query(
                "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (1, ?, ?)",
            )
            .bind(pos as i64)
            .bind(1 + pos % 3)
            .execute(&lib.state.pool)
            .await
            .unwrap();
        }
        let started = std::time::Instant::now();
        let (status, _, body) = get(&lib.app, "/api/playlists/1/export", "nas:8080").await;
        let took = started.elapsed();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.matches("#EXTINF").count(), 504);
        assert!(took < std::time::Duration::from_secs(1), "took {took:?}");
    }

    // ------------------------------------------------------------------
    // D2: Content-Disposition on /stream
    // ------------------------------------------------------------------

    async fn disposition_of(app: &Router, method: &str, uri: &str) -> String {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "{method} {uri}: {}",
            res.status()
        );
        res.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .expect("visible ASCII")
            .to_string()
    }

    #[tokio::test]
    async fn streams_name_the_track_with_the_rendition_extension() {
        let lib = library().await;
        for method in ["GET", "HEAD"] {
            assert_eq!(
                disposition_of(&lib.app, method, "/stream/1").await,
                "inline; filename=\"Miles Davis - So What.mp3\""
            );
            assert_eq!(
                disposition_of(&lib.app, method, "/stream/1?format=passthrough").await,
                "inline; filename=\"Miles Davis - So What.mp3\""
            );
            // A transcode is named for what it is, not for its source.
            assert_eq!(
                disposition_of(&lib.app, method, "/stream/3?format=flac").await,
                "inline; filename=\"Miles Davis - Blue in Green.flac\""
            );
            assert_eq!(
                disposition_of(&lib.app, method, "/stream/3?format=dop").await,
                "inline; filename=\"Miles Davis - Blue in Green.wav\""
            );
            // No tags: the fallback, with the source's own extension.
            assert_eq!(
                disposition_of(&lib.app, method, "/stream/6").await,
                "inline; filename=\"track-6.wav\""
            );
            // CJK: ASCII fallback plus the RFC 5987 form.
            assert_eq!(
                disposition_of(&lib.app, method, "/stream/7").await,
                "inline; filename=\"track-7.flac\"; filename*=UTF-8''\
                 %E5%9D%82%E6%9C%AC%E9%BE%8D%E4%B8%80%20-%20\
                 %E6%88%A6%E5%A0%B4%E3%81%AE%E3%83%A1%E3%83%AA%E3%83%BC\
                 %E3%82%AF%E3%83%AA%E3%82%B9%E3%83%9E%E3%82%B9.flac"
            );
        }
    }

    #[tokio::test]
    async fn ranged_and_untitled_by_artist_streams_are_named_too() {
        let lib = library().await;
        sqlx::query("UPDATE tracks SET artist = NULL WHERE id = 2")
            .execute(&lib.state.pool)
            .await
            .unwrap();
        let res = lib
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/stream/2")
                    .header(header::RANGE, "bytes=1-2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            res.headers()[header::CONTENT_DISPOSITION],
            "inline; filename=\"Freddie Freeloader.flac\""
        );
    }
}
