//! HTTP handlers: browse API, playlists, jobs, streaming.
//! (Spec §3.3, §3.4, §3.7, §3.8.)

use std::path::PathBuf;

use axum::{
    extract::{FromRequest, Path, Query, State},
    http::{header, HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use kahawai_core::{
    Album, Artist, AudioFormat, ImportPlaylistJson, ImportPlaylistResult, Job, JobKind, JobStatus,
    MusicError, NewPlaylist, Page, Playlist, PlaylistTracksMode, SetPlaylistTracks, StreamFormat,
    Track,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{db, jobs, scanner, stream, transcode, AppState};

/// Newtype so internal errors become JSON without leaking details.
pub struct ApiError(MusicError);

impl From<MusicError> for ApiError {
    fn from(e: MusicError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self.0 {
            MusicError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            MusicError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            MusicError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            MusicError::BadRange => (
                StatusCode::RANGE_NOT_SATISFIABLE,
                "unsatisfiable byte range".to_string(),
            ),
            MusicError::PayloadTooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, m.clone()),
            MusicError::UnsupportedFormat(m) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, m.clone()),
            MusicError::FeatureDisabled { feature, detail } => {
                // 501 with the cargo feature named, so clients (and the user)
                // know exactly which build flag is missing.
                let body = serde_json::json!({
                    "error": format!("feature disabled: {feature}"),
                    "feature": feature,
                    "detail": detail,
                });
                return (StatusCode::NOT_IMPLEMENTED, Json(body)).into_response();
            }
            other => {
                tracing::error!(error = %other, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// S10: canonicalize a catalog path and prove it stays under one of the
/// configured music roots. `canonicalize` resolves symlinks, so a symlink
/// pointing outside the library is rejected too. Rejections are
/// 404s, not 403s, to avoid leaking which paths exist. There is
/// deliberately no empty-roots bypass: serving a file with no configured
/// music root is a misconfiguration, not a default-open server.
fn ensure_within_roots(path: &std::path::Path, roots: &[PathBuf]) -> Result<PathBuf, MusicError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|_| MusicError::NotFound("track file missing".into()))?;
    for root in roots {
        if let Ok(canonical_root) = std::fs::canonicalize(root) {
            if canonical.starts_with(&canonical_root) {
                return Ok(canonical);
            }
        }
    }
    tracing::warn!(path = %path.display(), "served path escapes music roots");
    Err(MusicError::NotFound(
        "track file outside music library".into(),
    ))
}

/// Value of the `X-Gapless-Next` response header (S8): the track id the
/// client should play after this response. Present whenever `?next=`
/// named a real track, even when the server could not chain the audio
/// (passthrough, or a next track that resolves to a different rendition).
pub(crate) const GAPLESS_NEXT_HEADER: &str = "x-gapless-next";
/// Value of the `X-Gapless-Mode` response header (S8): `single-session`
/// or `chained`. Present only when the server actually chained audio.
pub(crate) const GAPLESS_MODE_HEADER: &str = "x-gapless-mode";

fn insert_gapless_next(res: &mut Response, next: Option<i64>) {
    if let Some(id) = next {
        res.headers_mut().insert(
            GAPLESS_NEXT_HEADER,
            id.to_string().parse().expect("numeric header"),
        );
    }
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// ---------------------------------------------------------------------------
// Browse: albums / artists / tracks / search  (S2)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PageQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_per_page")]
    pub per_page: u64,
}

fn default_page() -> u64 {
    1
}
fn default_per_page() -> u64 {
    100
}

fn album_from_row(r: &sqlx::sqlite::SqliteRow, track_count: u64) -> Album {
    Album {
        id: r.get("id"),
        title: r.get("title"),
        artist: r.get("artist"),
        year: r
            .get::<Option<i64>, _>("year")
            .and_then(|y| u16::try_from(y).ok()),
        artwork_hash: r.get("artwork_hash"),
        track_ids: Vec::new(),
        track_count,
    }
}

pub async fn list_albums(
    State(s): State<AppState>,
    Query(p): Query<PageQuery>,
) -> Result<Json<Page<Album>>, ApiError> {
    let page = p.page.max(1);
    let per_page = p.per_page.clamp(1, 500);
    let total: i64 = sqlx::query("SELECT COUNT(*) FROM albums")
        .fetch_one(&s.pool)
        .await
        .map_err(db::cvt)?
        .get(0);
    let rows = sqlx::query(
        "SELECT id, title, artist, year, artwork_hash FROM albums
         ORDER BY title LIMIT ? OFFSET ?",
    )
    .bind(per_page as i64)
    .bind(((page - 1) * per_page) as i64)
    .fetch_all(&s.pool)
    .await
    .map_err(db::cvt)?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: i64 = r.get("id");
        items.push(album_from_row(r, db::album_track_count(&s.pool, id).await?));
    }
    Ok(Json(Page {
        items,
        page,
        per_page,
        total: total as u64,
    }))
}

#[derive(Debug, Serialize)]
pub struct AlbumDetail {
    pub album: Album,
    pub tracks: Vec<Track>,
}

pub async fn get_album(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<AlbumDetail>, ApiError> {
    let r = sqlx::query("SELECT id, title, artist, year, artwork_hash FROM albums WHERE id = ?")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(db::cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("album {id}")))?;
    let tracks = db::tracks_for_album(&s.pool, id).await?;
    let track_count = tracks.len() as u64;
    let mut album = album_from_row(&r, track_count);
    album.track_ids = tracks.iter().map(|t| t.id).collect();
    Ok(Json(AlbumDetail { album, tracks }))
}

pub async fn list_artists(State(s): State<AppState>) -> Result<Json<Vec<Artist>>, ApiError> {
    let rows = sqlx::query("SELECT id, name FROM artists ORDER BY name")
        .fetch_all(&s.pool)
        .await
        .map_err(db::cvt)?;
    Ok(Json(
        rows.iter()
            .map(|r| Artist {
                id: r.get("id"),
                name: r.get("name"),
            })
            .collect(),
    ))
}

#[derive(Debug, Serialize)]
pub struct ArtistDetail {
    pub artist: Artist,
    pub albums: Vec<Album>,
}

pub async fn get_artist(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ArtistDetail>, ApiError> {
    let r = sqlx::query("SELECT id, name FROM artists WHERE id = ?")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(db::cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("artist {id}")))?;
    let artist = Artist {
        id,
        name: r.get("name"),
    };
    let album_rows = sqlx::query(
        "SELECT a.id, a.title, a.artist, a.year, a.artwork_hash
         FROM albums a JOIN album_artists aa ON aa.album_id = a.id
         WHERE aa.artist_id = ? ORDER BY a.title",
    )
    .bind(id)
    .fetch_all(&s.pool)
    .await
    .map_err(db::cvt)?;
    let mut albums = Vec::with_capacity(album_rows.len());
    for ar in &album_rows {
        let aid: i64 = ar.get("id");
        albums.push(album_from_row(
            ar,
            db::album_track_count(&s.pool, aid).await?,
        ));
    }
    Ok(Json(ArtistDetail { artist, albums }))
}

pub async fn get_track(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Track>, ApiError> {
    let track = db::get_track(&s.pool, id)
        .await?
        .ok_or_else(|| MusicError::NotFound(format!("track {id}")))?;
    Ok(Json(track))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

pub async fn search(
    State(s): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<Track>>, ApiError> {
    let matcher = build_fts_match(&q.q)
        .ok_or_else(|| MusicError::BadRequest("empty search query".to_string()))?;
    let rows =
        sqlx::query("SELECT rowid FROM search_fts WHERE search_fts MATCH ? ORDER BY rank LIMIT 50")
            .bind(&matcher)
            .fetch_all(&s.pool)
            .await
            .map_err(db::cvt)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: i64 = r.get(0);
        // Missing tracks stay out of search results; get_track still serves them.
        if let Some(t) = db::get_track(&s.pool, id).await? {
            if !t.missing {
                out.push(t);
            }
        }
    }
    Ok(Json(out))
}

/// Build a safe FTS5 MATCH expression: every whitespace-separated token
/// becomes a quoted prefix query, ANDed together. Returns `None` for an
/// empty query. Sanitizing to alphanumeric tokens keeps MATCH syntax errors
/// impossible by construction.
fn build_fts_match(q: &str) -> Option<String> {
    let tokens: Vec<String> = q
        .split_whitespace()
        .filter_map(|tok| {
            let clean: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if clean.is_empty() {
                None
            } else {
                Some(format!("\"{clean}\"*"))
            }
        })
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" AND "))
    }
}

// ---------------------------------------------------------------------------
// Playlists  (S7, S11, §3.8)
// ---------------------------------------------------------------------------

async fn playlist_by_id(s: &AppState, id: i64) -> Result<Playlist, ApiError> {
    let r = sqlx::query("SELECT id, name FROM playlists WHERE id = ?")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(db::cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("playlist {id}")))?;
    let track_rows =
        sqlx::query("SELECT track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position")
            .bind(id)
            .fetch_all(&s.pool)
            .await
            .map_err(db::cvt)?;
    Ok(Playlist {
        id,
        name: r.get("name"),
        track_ids: track_rows.iter().map(|t| t.get("track_id")).collect(),
    })
}

pub async fn get_playlist(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Playlist>, ApiError> {
    Ok(Json(playlist_by_id(&s, id).await?))
}

pub async fn list_playlists(State(s): State<AppState>) -> Result<Json<Vec<Playlist>>, ApiError> {
    let rows = sqlx::query("SELECT id FROM playlists ORDER BY id")
        .fetch_all(&s.pool)
        .await
        .map_err(db::cvt)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(playlist_by_id(&s, r.get("id")).await?);
    }
    Ok(Json(out))
}

pub async fn create_playlist(
    State(s): State<AppState>,
    Json(body): Json<NewPlaylist>,
) -> Result<Json<Playlist>, ApiError> {
    // S11: the queue lives client-side; `from_queue` just marks that
    // `queue_track_ids` carries the client's ordered queue.
    let track_ids: &[i64] = if body.from_queue {
        &body.queue_track_ids
    } else {
        &body.track_ids
    };
    let mut tx = s.pool.begin().await.map_err(db::cvt)?;
    let id: i64 = sqlx::query("INSERT INTO playlists (name) VALUES (?) RETURNING id")
        .bind(&body.name)
        .fetch_one(&mut *tx)
        .await
        .map_err(db::cvt)?
        .get("id");
    for (pos, tid) in track_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(pos as i64)
        .bind(tid)
        .execute(&mut *tx)
        .await
        .map_err(db::cvt)?;
    }
    tx.commit().await.map_err(db::cvt)?;
    Ok(Json(playlist_by_id(&s, id).await?))
}

pub async fn set_playlist_tracks(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<SetPlaylistTracks>,
) -> Result<Json<Playlist>, ApiError> {
    // 404 if the playlist doesn't exist.
    playlist_by_id(&s, id).await?;

    // Spec §3.8: expand album_ids in album track order, appended after track_ids.
    let mut track_ids = body.track_ids.clone();
    for album_id in &body.album_ids {
        let rows =
            sqlx::query("SELECT id FROM tracks WHERE album_id = ? ORDER BY disc_no, track_no, id")
                .bind(album_id)
                .fetch_all(&s.pool)
                .await
                .map_err(db::cvt)?;
        track_ids.extend(rows.iter().map(|r| r.get::<i64, _>("id")));
    }

    // S11: append (default) keeps existing entries and lands the new tracks
    // after them; replace wipes first. Positions stay dense either way.
    // One transaction: the playlist is never half-rewritten.
    let mut tx = s.pool.begin().await.map_err(db::cvt)?;
    let base: i64 = if body.mode == PlaylistTracksMode::Replace {
        sqlx::query("DELETE FROM playlist_tracks WHERE playlist_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db::cvt)?;
        -1
    } else {
        sqlx::query("SELECT COALESCE(MAX(position), -1) FROM playlist_tracks WHERE playlist_id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db::cvt)?
            .get(0)
    };
    for (i, tid) in track_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(base + 1 + i as i64)
        .bind(tid)
        .execute(&mut *tx)
        .await
        .map_err(db::cvt)?;
    }
    tx.commit().await.map_err(db::cvt)?;
    Ok(Json(playlist_by_id(&s, id).await?))
}

pub async fn delete_playlist(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let res = sqlx::query("DELETE FROM playlists WHERE id = ?")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(db::cvt)?;
    if res.rows_affected() == 0 {
        return Err(MusicError::NotFound(format!("playlist {id}")).into());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Rename body for `PATCH /api/playlists/{id}` (C3: the client needs
/// rename; the v1 API previously only had create/delete/replace-tracks).
#[derive(Debug, Deserialize)]
pub struct RenamePlaylist {
    pub name: String,
}

pub async fn rename_playlist(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<RenamePlaylist>,
) -> Result<Json<Playlist>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(MusicError::BadRequest("playlist name must not be empty".into()).into());
    }
    let res = sqlx::query("UPDATE playlists SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(db::cvt)?;
    if res.rows_affected() == 0 {
        return Err(MusicError::NotFound(format!("playlist {id}")).into());
    }
    Ok(Json(playlist_by_id(&s, id).await?))
}

/// One entry of an M3U/M3U8 playlist (S7 remainder).
struct M3uEntry {
    /// Raw path line, as written in the file.
    raw: String,
    /// `#EXTINF` title, when present. Informational only: matching is by
    /// canonical path, never by title/duration — no fuzzy fallback.
    #[allow(dead_code)]
    title: Option<String>,
}

/// Parse M3U/M3U8 text into entries in file order, honoring `#EXTINF`.
/// Blank lines, `#EXTM3U`, and other `#`-directives are skipped.
fn parse_m3u(text: &str) -> Vec<M3uEntry> {
    let mut entries = Vec::new();
    let mut pending_title: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            // Format: #EXTINF:<duration>,<title>
            pending_title = rest
                .split_once(',')
                .map(|(_, t)| t.trim().to_string())
                .filter(|t| !t.is_empty());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        entries.push(M3uEntry {
            raw: line.to_string(),
            title: pending_title.take(),
        });
    }
    entries
}

/// Resolve one M3U entry to a catalog track id by canonical path only.
/// Relative entries are tried against every configured music root; absolute
/// entries must canonicalize inside a root (S10: the playlist file is
/// client-supplied, so `../` escapes must not match). Returns `None` when
/// the file is missing or not in the catalog — the caller reports it in
/// `unmatched`.
async fn resolve_m3u_entry(s: &AppState, raw: &str) -> Result<Option<i64>, ApiError> {
    let entry_path = std::path::Path::new(raw);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if entry_path.is_absolute() {
        candidates.push(entry_path.to_path_buf());
    } else {
        for root in &s.music_dirs() {
            candidates.push(root.join(entry_path));
        }
    }
    for candidate in candidates {
        let Ok(canonical) = ensure_within_roots(&candidate, &s.music_dirs()) else {
            continue;
        };
        // The scanner stores walked paths; try the joined candidate verbatim
        // first, then the canonical form.
        for probe in [
            candidate.to_string_lossy().to_string(),
            canonical.to_string_lossy().to_string(),
        ] {
            let row = sqlx::query("SELECT id FROM tracks WHERE path = ?")
                .bind(&probe)
                .fetch_optional(&s.pool)
                .await
                .map_err(db::cvt)?;
            if let Some(r) = row {
                return Ok(Some(r.get("id")));
            }
        }
    }
    Ok(None)
}

/// Shared tail of both import inputs: parse, resolve, insert transactionally.
async fn finish_playlist_import(
    s: &AppState,
    name: Option<String>,
    data: &[u8],
) -> Result<Json<ImportPlaylistResult>, ApiError> {
    let text =
        std::str::from_utf8(data).map_err(|_| MusicError::BadRequest("not valid UTF-8".into()))?;
    let entries = parse_m3u(text);
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Imported playlist".to_string());

    let mut matched_ids = Vec::with_capacity(entries.len());
    let mut unmatched = Vec::new();
    for e in &entries {
        match resolve_m3u_entry(s, &e.raw).await? {
            Some(id) => matched_ids.push(id),
            None => unmatched.push(e.raw.clone()),
        }
    }

    let mut tx = s.pool.begin().await.map_err(db::cvt)?;
    let playlist_id: i64 = sqlx::query("INSERT INTO playlists (name) VALUES (?) RETURNING id")
        .bind(&name)
        .fetch_one(&mut *tx)
        .await
        .map_err(db::cvt)?
        .get("id");
    for (pos, tid) in matched_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (?, ?, ?)",
        )
        .bind(playlist_id)
        .bind(pos as i64)
        .bind(tid)
        .execute(&mut *tx)
        .await
        .map_err(db::cvt)?;
    }
    tx.commit().await.map_err(db::cvt)?;

    Ok(Json(ImportPlaylistResult {
        playlist_id,
        matched: matched_ids.len(),
        unmatched,
    }))
}

/// m3u/m3u8 playlist import (S7 remainder). Two inputs, one contract:
/// - JSON: `{ "name"?: string, "path": string }` — a server-local file
///   (v1 LAN trust).
/// - multipart: a `file` field (the .m3u/.m3u8 bytes) plus an optional
///   `name` text field.
///
/// Entries resolve to catalog tracks by canonical path only, in file order.
/// Unmatched entries are reported honestly; there is no title/duration
/// fallback. Bodies are capped at 10 MB (413).
pub async fn import_playlist(
    State(s): State<AppState>,
    req: Request<axum::body::Body>,
) -> Result<Json<ImportPlaylistResult>, ApiError> {
    const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;
    let too_large = || MusicError::PayloadTooLarge("playlist import capped at 10 MB".into());

    let is_multipart = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"));

    if is_multipart {
        let mut form = axum::extract::Multipart::from_request(req, &s)
            .await
            .map_err(|e| MusicError::BadRequest(format!("invalid multipart: {e}")))?;
        let mut name: Option<String> = None;
        let mut file_stem: Option<String> = None;
        let mut data: Option<Vec<u8>> = None;
        while let Some(field) = form
            .next_field()
            .await
            .map_err(|e| MusicError::BadRequest(format!("multipart field: {e}")))?
        {
            match field.name().unwrap_or("") {
                "name" => {
                    name = field
                        .text()
                        .await
                        .ok()
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty());
                }
                "file" => {
                    file_stem = field.file_name().map(|f| {
                        std::path::Path::new(f)
                            .file_stem()
                            .and_then(|x| x.to_str())
                            .unwrap_or("upload")
                            .to_string()
                    });
                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|e| MusicError::BadRequest(format!("multipart read: {e}")))?;
                    if bytes.len() > MAX_IMPORT_BYTES {
                        return Err(too_large().into());
                    }
                    data = Some(bytes.to_vec());
                }
                _ => {}
            }
        }
        let data = data.ok_or_else(|| {
            MusicError::BadRequest("multipart import needs a `file` field".into())
        })?;
        finish_playlist_import(&s, name.or(file_stem), &data).await
    } else {
        let bytes = axum::body::to_bytes(req.into_body(), MAX_IMPORT_BYTES)
            .await
            .map_err(|_| too_large())?;
        let req: ImportPlaylistJson = serde_json::from_slice(&bytes)
            .map_err(|e| MusicError::BadRequest(format!("invalid import JSON: {e}")))?;
        if req.path.trim().is_empty() {
            return Err(MusicError::BadRequest("import `path` must not be empty".into()).into());
        }
        // S10: the playlist file itself is a served file — it must live
        // under a music root, or its raw lines (returned as `unmatched`)
        // would become a filesystem-read oracle.
        let playlist_path = ensure_within_roots(std::path::Path::new(&req.path), &s.music_dirs())?;
        let data = std::fs::read(&playlist_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                MusicError::NotFound(format!("playlist file not found: {}", req.path))
            } else {
                MusicError::Io(e)
            }
        })?;
        if data.len() > MAX_IMPORT_BYTES {
            return Err(too_large().into());
        }
        let stem = std::path::Path::new(&req.path)
            .file_stem()
            .and_then(|x| x.to_str())
            .map(|x| x.to_string());
        finish_playlist_import(&s, req.name.or(stem), &data).await
    }
}

/// Artwork is addressed by the hash of its bytes, so a URL never changes
/// meaning: clients may cache it forever without revalidating.
const ARTWORK_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// Artwork by content hash, with ETag caching. (Spec §3.3, S2.)
pub async fn artwork(
    State(s): State<AppState>,
    Path(hash): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // S10: the hash is interpolated into the ETag and used as a DB key —
    // reject anything that is not a hex digest up front.
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(MusicError::BadRequest("artwork hash must be hexadecimal".into()).into());
    }
    let etag = format!("\"{hash}\"");
    if let Some(v) = headers.get(header::IF_NONE_MATCH) {
        if let Ok(v) = v.to_str() {
            if v.split(',').any(|t| t.trim() == etag || t.trim() == "*") {
                return Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .header(header::ETAG, etag)
                    .header(header::CACHE_CONTROL, ARTWORK_CACHE_CONTROL)
                    .body(axum::body::Body::empty())
                    .map_err(|e| ApiError(MusicError::Http(e.to_string())));
            }
        }
    }
    let row = sqlx::query("SELECT mime, bytes FROM artwork WHERE hash = ?")
        .bind(&hash)
        .fetch_optional(&s.pool)
        .await
        .map_err(db::cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("artwork {hash}")))?;
    let mime: String = row.get("mime");
    let bytes: Vec<u8> = row.get("bytes");
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, ARTWORK_CACHE_CONTROL)
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ApiError(MusicError::Http(e.to_string())))
}

// ---------------------------------------------------------------------------
// Library scan  (S1, §3.1)
// ---------------------------------------------------------------------------

/// Trigger a library scan as a durable job (S9). 202 Accepted immediately
/// with the job; progress = files_processed / files_total is visible via
/// `GET /api/jobs`, and the result message carries the scan counts. Only
/// one scan runs at a time (409 while one is in flight).
pub async fn trigger_scan(State(s): State<AppState>) -> Result<Response, ApiError> {
    let guard = s
        .scan_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| MusicError::Conflict("scan already running".to_string()))?;
    let job = s
        .jobs
        .create(JobKind::Scan, "Library scan".to_string(), None)
        .await;
    spawn_scan_job(s, job.clone(), guard);
    Ok((StatusCode::ACCEPTED, Json(job)).into_response())
}

/// Shared worker behind `POST /api/scan` and `POST /api/jobs` with
/// `kind: "scan"`. Holds `guard` so only one scan runs at a time.
/// Progress fans in through a channel: the scanner calls the callback per
/// file, and a forwarder task writes throttled absolute progress (≥1%
/// deltas, always the final tick) to the job store.
pub(crate) fn spawn_scan_job(s: AppState, job: Job, guard: tokio::sync::OwnedMutexGuard<()>) {
    tokio::spawn(async move {
        // Hold the guard for the whole scan so concurrent POSTs get 409.
        let _guard = guard;
        let jobs = s.jobs.clone();
        let job_id = job.id.clone();
        jobs.set_status(&job_id, JobStatus::Running).await;

        let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
        let jobs2 = jobs.clone();
        let job_id2 = job_id.clone();
        let fwd = tokio::spawn(async move {
            let mut last_written = -1.0f64;
            while let Some((done, estimate)) = prx.recv().await {
                // The total is only an estimate (last scan's count), so cap
                // below 1: finishing the job is what reports completion. A
                // first scan has no estimate and stays at 0 until it's done.
                let Some(total) = estimate else { continue };
                let p = (done as f64 / total as f64).clamp(0.0, 0.99);
                if p - last_written >= 0.01 {
                    last_written = p;
                    jobs2.set_progress(&job_id2, p as f32).await;
                }
            }
        });
        let report = scanner::run_scan_with_progress(&s.pool, &s.music_dirs(), |done, estimate| {
            let _ = ptx.send((done, estimate));
        })
        .await;
        // The progress closure only borrowed `ptx`, so the sender is still
        // alive here — drop it explicitly, otherwise the forwarder below
        // would wait forever for the channel to close.
        drop(ptx);
        // The callback owned the only sender; it is dropped now, so awaiting
        // the forwarder drains the remaining ticks before we finish the job.
        let _ = fwd.await;

        match report {
            Ok(r) => {
                tracing::info!(
                    added = r.files_added,
                    updated = r.files_updated,
                    skipped = r.files_skipped,
                    missing = r.files_missing,
                    "background scan complete"
                );
                jobs.finish(
                    &job_id,
                    true,
                    Some(format!(
                        "scan complete: {} files seen, {} added, {} updated, {} skipped, {} missing{}",
                        r.files_seen,
                        r.files_added,
                        r.files_updated,
                        r.files_skipped,
                        r.files_missing,
                        if r.walk_errors > 0 {
                            format!(", {} unreadable", r.walk_errors)
                        } else {
                            String::new()
                        }
                    )),
                )
                .await;
                // The catalog changed: tell already-connected clients (SSE)
                // rather than leaving them to notice on their next poll —
                // a scan triggered from the desktop app while a Player is
                // idle would otherwise never surface there. No receivers is
                // not an error; `send` just reports how many got it.
                let _ = s.catalog_events.send(crate::ServerEvent::CatalogUpdated);
                // Phase B: hash what the scan left pending.
                if let Err(e) = queue_hash_job(&s, "Content hashing").await {
                    tracing::warn!(error = %e, "could not queue content hashing");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "background scan failed");
                jobs.finish(&job_id, false, Some(e.to_string())).await;
            }
        }
    });
}

/// `GET /api/events`: an SSE stream emitting `catalog-updated` (a scan
/// finished) and `server-shutting-down` (the process is about to exit).
/// Routed outside the `api` sub-router (see `main.rs`) so the 60s API
/// timeout never closes it.
pub async fn scan_events(
    State(s): State<AppState>,
) -> axum::response::sse::Sse<
    impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use tokio_stream::StreamExt;
    let stream = tokio_stream::wrappers::BroadcastStream::new(s.catalog_events.subscribe())
        .filter_map(|msg| msg.ok())
        .map(|ev| {
            let name = match ev {
                crate::ServerEvent::CatalogUpdated => "catalog-updated",
                crate::ServerEvent::ShuttingDown => "server-shutting-down",
            };
            Ok(axum::response::sse::Event::default().event(name))
        });
    axum::response::sse::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

// ---------------------------------------------------------------------------
// Jobs  (S9, §3.7)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateJobBody {
    pub kind: JobKind,
    #[serde(default)]
    pub label: String,
    /// Server-local path, used by `extract_iso` jobs (the SACD ISO).
    #[serde(default)]
    pub path: Option<String>,
}

pub async fn list_jobs(State(s): State<AppState>) -> Json<Vec<Job>> {
    Json(s.jobs.list())
}

pub async fn get_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Job>, ApiError> {
    s.jobs
        .get(&id)
        .map(Json)
        .ok_or_else(|| MusicError::NotFound(format!("job {id}")).into())
}

pub async fn create_job(
    State(s): State<AppState>,
    Json(body): Json<CreateJobBody>,
) -> Result<Response, ApiError> {
    match body.kind {
        JobKind::ExtractIso => {
            // Honest stub (spec §2, S9): validate the ISO path, then fail
            // the job cleanly — the sacd_extract integration is a later
            // spike. Never shells out to anything.
            let iso = body.path.filter(|p| !p.trim().is_empty()).ok_or_else(|| {
                MusicError::BadRequest("extract_iso jobs require a server-local ISO `path`".into())
            })?;
            if !std::path::Path::new(&iso).is_file() {
                return Err(MusicError::NotFound(format!("ISO not found: {iso}")).into());
            }
            let job = s
                .jobs
                .create(JobKind::ExtractIso, body.label, Some(iso.clone()))
                .await;
            let store = s.jobs.clone();
            let id = job.id.clone();
            tokio::spawn(async move {
                store.set_status(&id, JobStatus::Running).await;
                store
                    .finish(
                        &id,
                        false,
                        Some(
                            "sacd_extract integration not yet implemented \
                             (spec §2: offline DSF extraction is a later spike); \
                             the ISO stays cataloged but undecodable"
                                .to_string(),
                        ),
                    )
                    .await;
            });
            Ok((StatusCode::ACCEPTED, Json(job)).into_response())
        }
        JobKind::Scan => {
            // Same worker as POST /api/scan: real scan, real progress.
            let guard = s
                .scan_lock
                .clone()
                .try_lock_owned()
                .map_err(|_| MusicError::Conflict("scan already running".to_string()))?;
            let job = s.jobs.create(JobKind::Scan, body.label, None).await;
            spawn_scan_job(s, job.clone(), guard);
            Ok((StatusCode::ACCEPTED, Json(job)).into_response())
        }
        JobKind::HashFiles => {
            let guard =
                s.hash_lock.clone().try_lock_owned().map_err(|_| {
                    MusicError::Conflict("content hashing already running".to_string())
                })?;
            let job = s.jobs.create(JobKind::HashFiles, body.label, None).await;
            spawn_hash_job(s, job.clone(), guard);
            Ok((StatusCode::ACCEPTED, Json(job)).into_response())
        }
        JobKind::Transcode => {
            let job = s.jobs.create(body.kind, body.label, None).await;
            // Demo worker: ticks progress to Done. Real bulk-transcode
            // workers will plug into the same JobStore lifecycle.
            let store = s.jobs.clone();
            let id = job.id.clone();
            tokio::spawn(jobs::run_ticker(
                store,
                id,
                20,
                0.05,
                std::time::Duration::from_millis(250),
            ));
            Ok(Json(job).into_response())
        }
    }
}

/// Queue the content-hashing job (Phase B) unless one is already running or
/// nothing is pending. Returns the job when one was queued.
pub(crate) async fn queue_hash_job(s: &AppState, label: &str) -> Result<Option<Job>, MusicError> {
    let Ok(guard) = s.hash_lock.clone().try_lock_owned() else {
        return Ok(None);
    };
    if crate::hashing::pending_count(&s.pool).await? == 0 {
        return Ok(None);
    }
    let job = s
        .jobs
        .create(JobKind::HashFiles, label.to_string(), None)
        .await;
    spawn_hash_job(s.clone(), job.clone(), guard);
    Ok(Some(job))
}

/// Worker for the `hash_files` job. Holds `guard` so only one runs at a time.
/// Progress is throttled to 1% steps like the scan's.
pub(crate) fn spawn_hash_job(s: AppState, job: Job, guard: tokio::sync::OwnedMutexGuard<()>) {
    tokio::spawn(async move {
        let _guard = guard;
        let jobs = s.jobs.clone();
        let job_id = job.id.clone();
        jobs.set_status(&job_id, JobStatus::Running).await;

        let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64)>();
        let jobs2 = jobs.clone();
        let job_id2 = job_id.clone();
        let fwd = tokio::spawn(async move {
            let mut last_written = -1.0f64;
            while let Some((done, total)) = prx.recv().await {
                if total == 0 {
                    continue;
                }
                let p = (done as f64 / total as f64).clamp(0.0, 0.99);
                if p - last_written >= 0.01 {
                    last_written = p;
                    jobs2.set_progress(&job_id2, p as f32).await;
                }
            }
        });
        let report = crate::hashing::hash_pending(&s.pool, |done, total| {
            let _ = ptx.send((done, total));
        })
        .await;
        drop(ptx);
        let _ = fwd.await;

        match report {
            Ok(r) => {
                let message = if r.skipped > 0 {
                    format!(
                        "hashed {} files; {} left pending (changed since the scan, or unreadable)",
                        r.hashed, r.skipped
                    )
                } else {
                    format!("hashed {} files", r.hashed)
                };
                jobs.finish(&job_id, true, Some(message)).await;
            }
            Err(e) => {
                tracing::error!(error = %e, "content hashing failed");
                jobs.finish(&job_id, false, Some(e.to_string())).await;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Streaming  (S3, S4, S6, S12, §3.4)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// Rendition selector (S13). Explicit `?format=` wins; absent → the
    /// server's `preferred_ladder` decides via [`kahawai_core::transcode_ladder`].
    #[serde(default)]
    pub format: Option<StreamFormat>,
    /// Transcode-side seek (S12). Ignored for passthrough: Range is the mechanism.
    pub seek_ms: Option<u64>,
    /// Gapless chaining (S8): the track id to serve after this one.
    /// Unknown/missing ids are 404, never silently dropped. Passthrough
    /// responses carry `X-Gapless-Next` for the client to honor; transcodes
    /// chain the audio in one response (`X-Gapless-Mode: single-session`
    /// or `chained`); DoP chains best-effort WAVs (`chained`).
    pub next: Option<i64>,
}

/// Value of the `X-Transcode-Chain` response header (S13): a short
/// human/machine-readable description of what produced the bytes,
/// e.g. `wav->passthrough` or `dsf64->flac 24/88.2`. Pure ASCII: header
/// values must survive `HeaderValue::to_str()`.
pub(crate) const TRANSCODE_CHAIN_HEADER: &str = "x-transcode-chain";

pub async fn stream_track(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let track = db::get_track(&s.pool, id)
        .await?
        .ok_or_else(|| MusicError::NotFound(format!("track {id}")))?;
    // S8: ?next= names a real track. Unknown/missing → 404, never silently
    // dropped. (HEAD validates the same way but reports current-track
    // metadata only — see stream_head.)
    let next_track = match q.next {
        Some(next_id) => Some(
            db::get_track(&s.pool, next_id)
                .await?
                .ok_or_else(|| MusicError::NotFound(format!("next track {next_id}")))?,
        ),
        None => None,
    };

    let fmt: AudioFormat = track.format;
    // S10: the catalog path must canonicalize inside a configured music root.
    let path = ensure_within_roots(std::path::Path::new(&track.path), &s.music_dirs())?;

    // Resolve the rendition (S13): explicit ?format= wins, otherwise the
    // server ladder. Ok(None) = passthrough. Errors: 501 for disabled
    // encoder features, 415 for SACD ISO, 400 for ?format=dop on a non-DSD
    // source. A plan with target Dop is routed to the DoP streamer (S5b),
    // never to the PCM transcode pipeline.
    let plan = transcode::resolve_plan(
        fmt,
        path.clone(),
        q.format,
        &s.preferred_ladder(),
        s.dsd_story(),
        q.seek_ms,
    )?;

    match plan {
        // `path` was canonicalized + root-checked above; the passthrough
        // open reuses it rather than re-reading the DB string.
        None => stream_passthrough(&track, fmt, &path, headers, next_track.map(|t| t.id)).await,
        Some(plan) if plan.target == StreamFormat::Dop => stream_dop(&s, plan, next_track).await,
        Some(plan) => stream_transcode(&s, plan, next_track).await,
    }
}

/// Byte-for-byte passthrough with full HTTP Range support (S3, S4).
/// `seek_ms` is ignored here (logged): Range is the seek mechanism.
/// S8: the client owns the handoff — the response only carries
/// `X-Gapless-Next` naming the next track.
async fn stream_passthrough(
    track: &Track,
    fmt: AudioFormat,
    path: &std::path::Path,
    headers: HeaderMap,
    next: Option<i64>,
) -> Result<Response, ApiError> {
    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(MusicError::NotFound(format!("file missing: {}", track.path)).into());
        }
        Err(e) => return Err(MusicError::Io(e).into()),
    };
    let total = file.metadata().await.map_err(MusicError::from)?.len();

    let range = match headers.get(header::RANGE) {
        Some(v) => {
            let v = v.to_str().map_err(|_| MusicError::BadRange)?;
            match stream::parse_range(v, total) {
                Ok(r) => Some(r),
                Err(_) => {
                    // 416 with the required Content-Range: bytes */total.
                    let mut res = (
                        StatusCode::RANGE_NOT_SATISFIABLE,
                        Json(serde_json::json!({ "error": "unsatisfiable byte range" })),
                    )
                        .into_response();
                    res.headers_mut().insert(
                        header::CONTENT_RANGE,
                        format!("bytes */{total}").parse().unwrap(),
                    );
                    insert_chain(&mut res, &transcode::passthrough_chain(fmt));
                    insert_gapless_next(&mut res, next);
                    return Ok(res);
                }
            }
        }
        None => None,
    };

    // Sanity: only serve formats we know are streamable as passthrough.
    // (The DB only contains scanned audio; this guards hand-inserted rows.)
    if !fmt.is_directly_streamable() {
        tracing::warn!(format = ?fmt, "refusing passthrough for non-streamable source");
        return Err(MusicError::UnsupportedFormat(format!("{fmt:?}")).into());
    }

    let mut res = stream::serve_file(file, total, range, fmt.mime_type()).await?;
    insert_chain(&mut res, &transcode::passthrough_chain(fmt));
    insert_gapless_next(&mut res, next);
    Ok(res)
}

/// DoP stream (S5b): DSD packed into 24-bit PCM frames in a WAV container
/// (see [`crate::dop`]). Unlike a live PCM transcode the bytes are fully
/// determined up front, so the response carries a real Content-Length.
/// `?seek_ms=` starts at the DoP frame boundary at or before the requested
/// time; a seeked stream carries no WAV header (it is the suffix of the
/// full pack starting at that frame). Deliberately no Accept-Ranges:
/// byte-range seeking is not supported on DoP in v1 — `?seek_ms=` is the
/// seek mechanism, mirroring the transcode path.
///
/// S8: `?next=` chains the next track's DoP stream (each with its own WAV
/// header) behind this one — best-effort, `X-Gapless-Mode: chained`. The
/// next track is forced onto DoP; a non-DSD next is a 400, never silently
/// dropped.
async fn stream_dop(
    s: &AppState,
    plan: transcode::TranscodePlan,
    next: Option<Track>,
) -> Result<Response, ApiError> {
    tracing::debug!(plan = %plan.describe(), "dop request");
    let next_id = next.as_ref().map(|t| t.id);
    let mut plans = vec![plan];
    if let Some(nt) = next {
        let nfmt: AudioFormat = nt.format;
        let npath = ensure_within_roots(std::path::Path::new(&nt.path), &s.music_dirs())?;
        let nplan = transcode::resolve_plan(
            nfmt,
            npath,
            Some(StreamFormat::Dop),
            &s.preferred_ladder(),
            s.dsd_story(),
            None,
        )?
        .ok_or_else(|| MusicError::BadRequest("next track does not resolve to DoP".into()))?;
        debug_assert_eq!(nplan.target, StreamFormat::Dop);
        plans.push(nplan);
    }
    let chained = plans.len() > 1;

    let (chains, total_len, body) = tokio::task::spawn_blocking(move || {
        let mut chains: Vec<String> = Vec::with_capacity(plans.len());
        let mut total_len: u64 = 0;
        let mut streamers = Vec::with_capacity(plans.len());
        for plan in plans {
            let file = std::fs::File::open(&plan.path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    MusicError::NotFound(format!("file missing: {}", plan.path.display()))
                } else {
                    MusicError::Io(e)
                }
            })?;
            let (dop_plan, reader) = crate::dop::DopPlan::resolve(file, plan.source_format)?;
            let mut streamer = crate::dop::DopStreamer::new(reader, dop_plan);
            let frame = plan.seek_ms.map(|ms| dop_plan.seek_frame(ms)).unwrap_or(0);
            streamer.seek_frame(frame)?;
            total_len += dop_plan.seeked_len(frame);
            chains.push(dop_plan.chain_label());
            streamers.push(streamer);
        }
        let body = crate::dop::dop_body_chained(streamers);
        Ok::<_, MusicError>((chains, total_len, body))
    })
    .await
    .map_err(|e| MusicError::Http(format!("dop setup panicked: {e}")))??;

    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, StreamFormat::Dop.mime_type())
        .header(header::CONTENT_LENGTH, total_len)
        .body(body)
        .map_err(|e| ApiError(MusicError::Http(e.to_string())))?;
    insert_chain(&mut res, &chains.join(" + "));
    insert_gapless_next(&mut res, next_id);
    if chained {
        res.headers_mut().insert(
            GAPLESS_MODE_HEADER,
            transcode::GaplessMode::Chained
                .header_value()
                .parse()
                .expect("static header"),
        );
    }
    Ok(res)
}

/// Transcoded stream (S6): blocking pipeline → chunked body, no
/// Content-Length, `seek_ms` applied sample-exact before encoding (S12).
///
/// S8: `?next=` chains the next track into the same response. The next
/// track inherits the current track's resolved target (it never re-runs
/// the ladder); `seek_ms` applies to the first track only. Identical PCM
/// specs → one encoder session (`X-Gapless-Mode: single-session`);
/// differing specs → concatenated encoder sessions (`chained`). A next
/// track that resolves to passthrough can't mix into a transcode — the
/// current track is served alone, but `X-Gapless-Next` still names it.
async fn stream_transcode(
    s: &AppState,
    plan: transcode::TranscodePlan,
    next: Option<Track>,
) -> Result<Response, ApiError> {
    tracing::debug!(plan = %plan.describe(), "transcode request");
    let next_id = next.as_ref().map(|t| t.id);
    let mut plans = vec![plan];
    if let Some(nt) = next {
        let nfmt: AudioFormat = nt.format;
        let npath = ensure_within_roots(std::path::Path::new(&nt.path), &s.music_dirs())?;
        match transcode::resolve_plan(
            nfmt,
            npath.clone(),
            Some(plans[0].target),
            &s.preferred_ladder(),
            s.dsd_story(),
            None,
        )? {
            Some(nplan) => plans.push(nplan),
            // S8: resolve_plan returns None when the next track is already
            // in the target format (e.g. FLAC→FLAC is a no-op passthrough).
            // But the ?next= chain was explicitly requested — force a
            // decode+re-encode so the next track joins the gapless encoder
            // session instead of the chain being silently reduced to the
            // current track alone. (Re-encoding FLAC→FLAC is wasteful but
            // bit-correct: the pipeline decodes to PCM and re-encodes.)
            None => plans.push(transcode::TranscodePlan {
                path: npath,
                source_format: nfmt,
                target: plans[0].target,
                seek_ms: None,
            }),
        }
    }

    if plans.len() == 1 {
        let prepared =
            tokio::task::spawn_blocking(move || transcode::PreparedTranscode::setup(&plans[0]))
                .await
                .map_err(|e| MusicError::Http(format!("transcode setup panicked: {e}")))??;
        let chain = prepared.chain.clone();
        let content_type = prepared.content_type;
        let body = transcode::transcode_body(prepared);

        let mut res = Response::builder()
            .header(header::CONTENT_TYPE, content_type)
            // Deliberately no Content-Length (unknown until the encode
            // finishes) and no Accept-Ranges: a live transcode cannot be
            // ranged. Axum will use chunked transfer encoding.
            .body(body)
            .map_err(|e| ApiError(MusicError::Http(e.to_string())))?;
        insert_chain(&mut res, &chain);
        insert_gapless_next(&mut res, next_id);
        return Ok(res);
    }

    let prepared = tokio::task::spawn_blocking(move || transcode::PreparedGapless::setup(&plans))
        .await
        .map_err(|e| MusicError::Http(format!("gapless setup panicked: {e}")))??;
    let mode = prepared.mode;
    let chain = prepared.chain.clone();
    let content_type = prepared.content_type;
    let body = transcode::gapless_body(prepared);

    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(body)
        .map_err(|e| ApiError(MusicError::Http(e.to_string())))?;
    insert_chain(&mut res, &chain);
    insert_gapless_next(&mut res, next_id);
    res.headers_mut().insert(
        GAPLESS_MODE_HEADER,
        mode.header_value().parse().expect("static header"),
    );
    Ok(res)
}

fn insert_chain(res: &mut Response, chain: &str) {
    res.headers_mut().insert(
        TRANSCODE_CHAIN_HEADER,
        chain.parse().unwrap_or_else(|_| {
            // Header values must be visible ASCII; fall back to a tag.
            "transcode".parse().unwrap()
        }),
    );
}

/// HEAD: headers for duration probing without the body. (Spec §3.4.)
/// HEAD: headers for duration probing without the body. (Spec §3.4.)
/// Resolves the same rendition GET would serve so the metadata agrees:
/// passthrough reports the source file; a transcode reports the target MIME
/// with no Accept-Ranges (like GET — a live transcode cannot be ranged).
/// hyper synthesizes `content-length: 0` for the empty HEAD body itself.
/// Every response carries `X-Transcode-Chain` (S13).
pub async fn stream_head(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<StreamQuery>,
) -> Result<Response, ApiError> {
    let track = db::get_track(&s.pool, id)
        .await?
        .ok_or_else(|| MusicError::NotFound(format!("track {id}")))?;
    // S8: validate ?next= the same way GET does (unknown → 404), but HEAD
    // reports the current track's metadata only — it never chains audio.
    if let Some(next_id) = q.next {
        db::get_track(&s.pool, next_id)
            .await?
            .ok_or_else(|| MusicError::NotFound(format!("next track {next_id}")))?;
    }
    let fmt: AudioFormat = track.format;
    let path = ensure_within_roots(std::path::Path::new(&track.path), &s.music_dirs())?;
    let plan = transcode::resolve_plan(
        fmt,
        path.clone(),
        q.format,
        &s.preferred_ladder(),
        s.dsd_story(),
        q.seek_ms,
    )?;

    match plan {
        None => {
            let total = tokio::fs::metadata(&path)
                .await
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        MusicError::NotFound(format!("file missing: {}", track.path))
                    } else {
                        MusicError::Io(e)
                    }
                })?
                .len();
            let mut res = Response::builder()
                .header(header::CONTENT_TYPE, fmt.mime_type())
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, total)
                .body(axum::body::Body::empty())
                .map_err(|e| ApiError(MusicError::Http(e.to_string())))?;
            insert_chain(&mut res, &transcode::passthrough_chain(fmt));
            Ok(res)
        }
        Some(plan) => {
            // Mirror GET's metadata minus the body: target MIME, no
            // Accept-Ranges. DoP reports the real Content-Length (known up
            // front, S5b); a live PCM transcode has none (unknown until
            // encoded). Opening source headers is blocking IO →
            // spawn_blocking.
            let (chain, content_type, content_len) =
                tokio::task::spawn_blocking(move || transcode::head_transcode_meta(&plan))
                    .await
                    .map_err(|e| {
                        MusicError::Http(format!("head transcode meta panicked: {e}"))
                    })??;
            let mut builder = Response::builder().header(header::CONTENT_TYPE, content_type);
            if let Some(len) = content_len {
                builder = builder.header(header::CONTENT_LENGTH, len);
            }
            let mut res = builder
                .body(axum::body::Body::empty())
                .map_err(|e| ApiError(MusicError::Http(e.to_string())))?;
            insert_chain(&mut res, &chain);
            Ok(res)
        }
    }
}
