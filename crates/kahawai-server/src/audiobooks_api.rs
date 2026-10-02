//! HTTP side of audiobooks (docs/v1/kahawai-audiobook-spec.md): the folders,
//! the library and book detail, and the position, bookmark, history and
//! settings stores. All positions are `book_offset_ms`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use kahawai_core::{JobKind, JobStatus, MusicError};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, Row, SqlitePool};

use crate::api::ApiError;
use crate::audiobooks::{self as ab, now_ms};
use crate::db::cvt;
use crate::AppState;

type ApiResult<T> = Result<T, ApiError>;

// --- listeners ----------------------------------------------------------------
//
// Several people can share one server: each *listener* has their own place in
// every book, bookmarks, history, finished flags and speed, while the library
// and its details are shared. There are no accounts (the server is for a
// trusted home network): a listener is a name the Player sends in
// `X-Kahawai-Listener` (percent-encoded). No name, or "Default", is listener 0.

pub const LISTENER_HEADER: &str = "x-kahawai-listener";
const MAX_LISTENER_NAME: usize = 40;

/// Who is asking: the listener's id.
pub struct Listener(pub i64);

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A listener's name as stored: trimmed, one line, a sensible length.
pub fn clean_listener_name(raw: &str) -> Result<String, MusicError> {
    let n = raw.trim();
    if n.is_empty() {
        return Err(MusicError::BadRequest("a listener needs a name".into()));
    }
    if n.chars().count() > MAX_LISTENER_NAME || n.chars().any(char::is_control) {
        return Err(MusicError::BadRequest(format!(
            "a listener's name is one line of at most {MAX_LISTENER_NAME} characters"
        )));
    }
    Ok(n.to_string())
}

/// The id for a listener name, creating the listener the first time.
pub async fn listener_id(pool: &SqlitePool, name: &str) -> Result<i64, MusicError> {
    let name = clean_listener_name(name)?;
    if let Some(r) = sqlx::query("SELECT id FROM audiobook_listeners WHERE name = ?")
        .bind(&name)
        .fetch_optional(pool)
        .await
        .map_err(cvt)?
    {
        return Ok(r.get(0));
    }
    let id: i64 = sqlx::query("INSERT INTO audiobook_listeners (name) VALUES (?) ON CONFLICT(name) DO UPDATE SET name = name RETURNING id")
        .bind(&name)
        .fetch_one(pool)
        .await
        .map_err(cvt)?
        .get(0);
    Ok(id)
}

impl axum::extract::FromRequestParts<AppState> for Listener {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get(LISTENER_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(percent_decode)
            .unwrap_or_default();
        if raw.trim().is_empty() {
            return Ok(Listener(0));
        }
        Ok(Listener(listener_id(&state.pool, &raw).await?))
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ListenerInfo {
    pub id: i64,
    pub name: String,
    /// Books this listener has a place in.
    pub books_started: i64,
}

pub async fn list_listeners(State(s): State<AppState>) -> ApiResult<Json<Vec<ListenerInfo>>> {
    Ok(Json(
        sqlx::query(
            "SELECT l.id, l.name,
                    (SELECT COUNT(*) FROM audiobook_positions p WHERE p.user_id = l.id) AS started
             FROM audiobook_listeners l ORDER BY l.id = 0 DESC, l.name COLLATE NOCASE",
        )
        .fetch_all(&s.pool)
        .await
        .map_err(cvt)?
        .iter()
        .map(|r| ListenerInfo {
            id: r.get("id"),
            name: r.get("name"),
            books_started: r.get("started"),
        })
        .collect(),
    ))
}

#[derive(Deserialize)]
pub struct NewListener {
    pub name: String,
}

/// `POST /api/audiobook-listeners`: make a listener (also made the first time a name is used).
pub async fn add_listener(
    State(s): State<AppState>,
    Json(b): Json<NewListener>,
) -> ApiResult<Response> {
    let id = listener_id(&s.pool, &b.name).await?;
    let name = clean_listener_name(&b.name)?;
    Ok((
        StatusCode::CREATED,
        Json(ListenerInfo {
            id,
            name,
            books_started: 0,
        }),
    )
        .into_response())
}

/// `DELETE /api/audiobook-listeners/{id}`: forget a listener and everything
/// that is theirs (the library is untouched). The default listener stays.
pub async fn delete_listener(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    if id == 0 {
        return Err(MusicError::BadRequest("the default listener cannot be removed".into()).into());
    }
    let mut tx = s.pool.begin().await.map_err(cvt)?;
    let found = sqlx::query("SELECT 1 FROM audiobook_listeners WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(cvt)?;
    if found.is_none() {
        return Err(MusicError::NotFound(format!("listener {id}")).into());
    }
    for table in [
        "audiobook_bookmarks",
        "audiobook_sessions",
        "audiobook_positions",
        "audiobook_settings",
        "audiobook_finished",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE user_id = ?"))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(cvt)?;
    }
    sqlx::query("DELETE FROM audiobook_listeners WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(cvt)?;
    tx.commit().await.map_err(cvt)?;
    Ok(StatusCode::NO_CONTENT)
}

// --- roots ------------------------------------------------------------------

pub async fn list_roots(State(s): State<AppState>) -> ApiResult<Json<Vec<ab::Root>>> {
    Ok(Json(ab::list_roots(&s.pool).await?))
}

#[derive(Deserialize)]
pub struct NewRoot {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Register a folder and start scanning it in the background.
pub async fn add_root(State(s): State<AppState>, Json(b): Json<NewRoot>) -> ApiResult<Response> {
    let root = ab::add_root(&s.pool, &b.path, b.name.as_deref()).await?;
    // Best effort: if a scan is already running, the next one picks it up.
    if let Ok(guard) = s.scan_lock.clone().try_lock_owned() {
        let job = s
            .jobs
            .create(JobKind::Scan, "Audiobook scan".to_string(), None)
            .await;
        spawn_audiobook_scan(s, job.id, guard);
    }
    Ok((StatusCode::CREATED, Json(root)).into_response())
}

pub async fn delete_root(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    ab::delete_root(&s.pool, id).await?;
    let _ = s.catalog_events.send(crate::ServerEvent::CatalogUpdated);
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/audiobooks/scan`: rescan the audiobook folders as a job.
pub async fn trigger_scan(State(s): State<AppState>) -> ApiResult<Response> {
    let guard = s
        .scan_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| MusicError::Conflict("scan already running".to_string()))?;
    let job = s
        .jobs
        .create(JobKind::Scan, "Audiobook scan".to_string(), None)
        .await;
    spawn_audiobook_scan(s, job.id.clone(), guard);
    Ok((StatusCode::ACCEPTED, Json(job)).into_response())
}

pub(crate) fn spawn_audiobook_scan(
    s: AppState,
    job_id: String,
    guard: tokio::sync::OwnedMutexGuard<()>,
) {
    tokio::spawn(async move {
        let _guard = guard;
        s.jobs.set_status(&job_id, JobStatus::Running).await;
        // Live file counts, rate and ETA, like the music scan's (about once a second).
        let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
        let (jobs, id2) = (s.jobs.clone(), job_id.clone());
        let fwd = tokio::spawn(async move {
            let mut rate = crate::jobs::RateTracker::default();
            let mut every = crate::jobs::Every::new(1000);
            while let Some((done, estimate)) = prx.recv().await {
                let now = crate::jobs::now_ms();
                if every.due(now) {
                    jobs.set_files(&id2, rate.observe(now, done, None, estimate))
                        .await;
                    if let Some(total) = estimate {
                        jobs.set_progress(
                            &id2,
                            (done as f64 / total as f64).clamp(0.0, 0.99) as f32,
                        )
                        .await;
                    }
                }
            }
        });
        let report = ab::scan_with_progress(
            &s.pool,
            std::sync::Arc::new(move |done, estimate| {
                let _ = ptx.send((done, estimate));
            }),
        )
        .await;
        let _ = fwd.await;
        // Phase two: spot copies of books by content (cheap: only files that
        // share a size with another are read).
        s.jobs
            .set_message(&job_id, Some("Checking for duplicate copies…".to_string()))
            .await;
        let report = match report {
            Ok(mut r) => match ab::find_duplicates(&s.pool).await {
                Ok(n) => {
                    r.duplicates = n;
                    Ok(r)
                }
                Err(e) => Err(e),
            },
            e => e,
        };
        match report {
            Ok(r) => {
                s.jobs
                    .finish(
                        &job_id,
                        true,
                        Some(format!(
                            "audiobook scan complete: {} books, {} files{}",
                            r.books,
                            r.files,
                            if r.duplicates > 0 {
                                format!(", {} duplicate copies hidden", r.duplicates)
                            } else {
                                String::new()
                            }
                        )),
                    )
                    .await;
                let _ = s.catalog_events.send(crate::ServerEvent::CatalogUpdated);
            }
            Err(e) => {
                s.jobs.finish(&job_id, false, Some(e.to_string())).await;
            }
        }
    });
}

#[derive(Deserialize, Default)]
pub struct EnrichBody {
    /// One book (asked again even if it was asked before); default: every
    /// book with a blank that has not been asked about.
    #[serde(default)]
    pub book_id: Option<i64>,
}

/// `POST /api/audiobooks/enrich`: look up the author, year and cover of books
/// that lack them (Open Library, then Google Books). Sends titles off the
/// LAN, so it needs the album lookup's opt-in (`enrichment_enabled`).
pub async fn enrich(
    State(s): State<AppState>,
    Listener(uid): Listener,
    body: Option<Json<EnrichBody>>,
) -> ApiResult<Response> {
    if !s.config.read().unwrap().enrichment_enabled {
        return Err(MusicError::BadRequest(
            "online lookup is off: turn on Album info in the Server settings first".to_string(),
        )
        .into());
    }
    let only = body.and_then(|b| b.0.book_id);
    if let Some(id) = only {
        get_book(&s.pool, id, uid).await?;
    }
    let guard = s
        .enrich_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| MusicError::Conflict("a lookup is already running".to_string()))?;
    let job = s
        .jobs
        .create(
            JobKind::EnrichBooks,
            "Audiobook info lookup".to_string(),
            None,
        )
        .await;
    spawn_enrich(
        s,
        job.id.clone(),
        only,
        guard,
        crate::book_meta::BookClient::new(),
    );
    Ok((StatusCode::ACCEPTED, Json(job)).into_response())
}

pub(crate) fn spawn_enrich(
    s: AppState,
    job_id: String,
    only: Option<i64>,
    guard: tokio::sync::OwnedMutexGuard<()>,
    client: crate::book_meta::BookClient,
) {
    tokio::spawn(async move {
        let _guard = guard;
        s.jobs.set_status(&job_id, JobStatus::Running).await;
        let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64)>();
        let jobs = s.jobs.clone();
        let id2 = job_id.clone();
        let fwd = tokio::spawn(async move {
            while let Some((done, total)) = prx.recv().await {
                if total > 0 {
                    jobs.set_progress(&id2, (done as f64 / total as f64).clamp(0.0, 0.99) as f32)
                        .await;
                }
            }
        });
        let report =
            crate::book_meta::enrich_books(&s.pool, &client, only, &s.jobs, &job_id, |d, t| {
                let _ = ptx.send((d, t));
            })
            .await;
        drop(ptx);
        let _ = fwd.await;
        match report {
            Ok(r) if r.cancelled => {}
            Ok(r) => {
                let msg = match &r.offline {
                    Some(why) => format!(
                        "stopped: {why}; {} books done, the rest will be tried next time",
                        r.looked_up
                    ),
                    None => format!(
                        "looked up {} books: {} matched ({} covers), {} not found",
                        r.looked_up, r.matched, r.covers, r.not_found
                    ),
                };
                s.jobs.finish(&job_id, r.offline.is_none(), Some(msg)).await;
                if r.matched > 0 {
                    let _ = s.catalog_events.send(crate::ServerEvent::CatalogUpdated);
                }
            }
            Err(e) => {
                s.jobs.finish(&job_id, false, Some(e.to_string())).await;
            }
        }
    });
}

// --- books ------------------------------------------------------------------

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Book {
    pub id: i64,
    pub root_id: i64,
    pub title: String,
    pub author: Option<String>,
    pub narrator: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub year: Option<i64>,
    pub cover_hash: Option<String>,
    pub duration_ms: i64,
    pub added_at: i64,
    pub finished_at: Option<i64>,
    /// Where the listener is, and when that was last saved.
    pub position_ms: i64,
    pub last_played_at: Option<i64>,
    /// `position_ms / duration_ms`, 0 to 1.
    pub progress: f64,
}

const BOOK_COLS: &str = "b.id, b.root_id, b.title, b.author, b.narrator, b.series, b.series_index,
     b.year, b.cover_hash, b.duration_ms, b.added_at, f.finished_at,
     COALESCE(p.book_offset_ms, 0) AS position_ms, p.updated_at AS last_played_at";

/// The joins that bring in one listener's place and finished flag. `uid` is a
/// number we got from the database, never text from the request.
fn listener_joins(uid: i64) -> String {
    format!(
        "LEFT JOIN audiobook_positions p ON p.book_id = b.id AND p.user_id = {uid}
         LEFT JOIN audiobook_finished f ON f.book_id = b.id AND f.user_id = {uid}"
    )
}

/// A book is listed while at least one of its files is on disk.
const BOOK_PRESENT: &str =
    "EXISTS (SELECT 1 FROM audiobook_parts ap JOIN tracks t ON t.id = ap.track_id
     WHERE ap.book_id = b.id AND t.missing = 0)";

fn book_from_row(r: &SqliteRow) -> Book {
    let duration: i64 = r.get("duration_ms");
    let position: i64 = r.get("position_ms");
    Book {
        id: r.get("id"),
        root_id: r.get("root_id"),
        title: r.get("title"),
        author: r.get("author"),
        narrator: r.get("narrator"),
        series: r.get("series"),
        series_index: r.get("series_index"),
        year: r.get("year"),
        cover_hash: r.get("cover_hash"),
        duration_ms: duration,
        added_at: r.get("added_at"),
        finished_at: r.get("finished_at"),
        position_ms: position,
        last_played_at: r.get("last_played_at"),
        progress: if duration > 0 {
            (position as f64 / duration as f64).clamp(0.0, 1.0)
        } else {
            0.0
        },
    }
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    /// `continue`: books you have started, unfinished first, newest first.
    pub shelf: Option<String>,
    pub q: Option<String>,
    pub author: Option<String>,
    pub series: Option<String>,
    pub finished: Option<bool>,
}

pub async fn list_books(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Book>>> {
    Ok(Json(books(&s.pool, &q, uid).await?))
}

pub async fn books(pool: &SqlitePool, q: &ListQuery, uid: i64) -> Result<Vec<Book>, MusicError> {
    let mut sql = format!(
        "SELECT {BOOK_COLS} FROM audiobooks b
         {}
         WHERE {BOOK_PRESENT} AND b.duplicate_of IS NULL",
        listener_joins(uid)
    );
    let mut binds: Vec<String> = Vec::new();
    let continue_shelf = q.shelf.as_deref() == Some("continue");
    if continue_shelf {
        sql.push_str(" AND p.book_id IS NOT NULL");
    }
    if let Some(text) = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        sql.push_str(" AND (b.title LIKE ? ESCAPE '\\' OR b.author LIKE ? ESCAPE '\\' OR b.narrator LIKE ? ESCAPE '\\' OR b.series LIKE ? ESCAPE '\\')");
        let like = format!(
            "%{}%",
            text.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        binds.extend(std::iter::repeat_n(like, 4));
    }
    if let Some(a) = q.author.as_deref().filter(|a| !a.is_empty()) {
        sql.push_str(" AND b.author = ?");
        binds.push(a.to_string());
    }
    if let Some(sr) = q.series.as_deref().filter(|a| !a.is_empty()) {
        sql.push_str(" AND b.series = ?");
        binds.push(sr.to_string());
    }
    match q.finished {
        Some(true) => sql.push_str(" AND f.finished_at IS NOT NULL"),
        Some(false) => sql.push_str(" AND f.finished_at IS NULL"),
        None => {}
    }
    if continue_shelf {
        sql.push_str(" ORDER BY (f.finished_at IS NOT NULL), p.updated_at DESC, b.id");
    } else {
        sql.push_str(" ORDER BY COALESCE(b.author, '') COLLATE NOCASE, COALESCE(b.series, '') COLLATE NOCASE, COALESCE(b.series_index, 0), b.title COLLATE NOCASE, b.id");
    }
    let mut query = sqlx::query(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rows = query.fetch_all(pool).await.map_err(cvt)?;
    Ok(rows.iter().map(book_from_row).collect())
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Part {
    pub id: i64,
    pub track_id: i64,
    pub part_index: i64,
    pub title: Option<String>,
    pub start_offset_ms: i64,
    pub duration_ms: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Chapter {
    pub id: i64,
    pub part_id: i64,
    pub title: String,
    pub start_offset_ms: i64,
    pub duration_ms: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub id: i64,
    pub book_id: i64,
    pub book_offset_ms: i64,
    pub name: String,
    pub note: String,
    pub created_at: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Settings {
    pub speed: f64,
    pub skip_back_s: i64,
    pub skip_forward_s: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed: 1.0,
            skip_back_s: 15,
            skip_forward_s: 30,
        }
    }
}

#[derive(Serialize)]
pub struct BookDetail {
    #[serde(flatten)]
    pub book: Book,
    pub parts: Vec<Part>,
    pub chapters: Vec<Chapter>,
    pub bookmarks: Vec<Bookmark>,
    pub settings: Settings,
}

fn bookmark_from_row(r: &SqliteRow) -> Bookmark {
    Bookmark {
        id: r.get("id"),
        book_id: r.get("book_id"),
        book_offset_ms: r.get("book_offset_ms"),
        name: r.get("name"),
        note: r.get("note"),
        created_at: r.get("created_at"),
    }
}

async fn get_book(pool: &SqlitePool, id: i64, uid: i64) -> Result<Book, MusicError> {
    let sql = format!(
        "SELECT {BOOK_COLS} FROM audiobooks b {} WHERE b.id = ?",
        listener_joins(uid)
    );
    sqlx::query(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(cvt)?
        .map(|r| book_from_row(&r))
        .ok_or_else(|| MusicError::NotFound(format!("audiobook {id}")))
}

pub async fn get_settings(pool: &SqlitePool, id: i64, uid: i64) -> Result<Settings, MusicError> {
    Ok(sqlx::query(
        "SELECT speed, skip_back_s, skip_forward_s FROM audiobook_settings WHERE book_id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(uid)
    .fetch_optional(pool)
    .await
    .map_err(cvt)?
    .map(|r| Settings {
        speed: r.get("speed"),
        skip_back_s: r.get("skip_back_s"),
        skip_forward_s: r.get("skip_forward_s"),
    })
    .unwrap_or_default())
}

pub async fn book_detail(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
) -> ApiResult<Json<BookDetail>> {
    let book = get_book(&s.pool, id, uid).await?;
    let parts = parts_of(&s.pool, id).await?;
    let chapters = sqlx::query(
        "SELECT id, part_id, title, start_offset_ms, duration_ms FROM audiobook_chapters
         WHERE book_id = ? ORDER BY start_offset_ms, id",
    )
    .bind(id)
    .fetch_all(&s.pool)
    .await
    .map_err(cvt)?
    .iter()
    .map(|r| Chapter {
        id: r.get("id"),
        part_id: r.get("part_id"),
        title: r.get("title"),
        start_offset_ms: r.get("start_offset_ms"),
        duration_ms: r.get("duration_ms"),
    })
    .collect();
    let bookmarks = bookmarks_of(&s.pool, id, uid).await?;
    let settings = get_settings(&s.pool, id, uid).await?;
    Ok(Json(BookDetail {
        book,
        parts,
        chapters,
        bookmarks,
        settings,
    }))
}

async fn parts_of(pool: &SqlitePool, id: i64) -> Result<Vec<Part>, MusicError> {
    Ok(sqlx::query(
        "SELECT id, track_id, part_index, title, start_offset_ms, duration_ms FROM audiobook_parts
         WHERE book_id = ? ORDER BY part_index",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(cvt)?
    .iter()
    .map(|r| Part {
        id: r.get("id"),
        track_id: r.get("track_id"),
        part_index: r.get("part_index"),
        title: r.get("title"),
        start_offset_ms: r.get("start_offset_ms"),
        duration_ms: r.get("duration_ms"),
    })
    .collect())
}

async fn bookmarks_of(pool: &SqlitePool, id: i64, uid: i64) -> Result<Vec<Bookmark>, MusicError> {
    Ok(sqlx::query(
        "SELECT id, book_id, book_offset_ms, name, note, created_at FROM audiobook_bookmarks
         WHERE book_id = ? AND user_id = ? ORDER BY book_offset_ms, id",
    )
    .bind(id)
    .bind(uid)
    .fetch_all(pool)
    .await
    .map_err(cvt)?
    .iter()
    .map(bookmark_from_row)
    .collect())
}

// --- metadata edit ----------------------------------------------------------

#[derive(Deserialize)]
pub struct MetaEdit {
    pub title: Option<String>,
    pub author: Option<String>,
    pub narrator: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub year: Option<i64>,
}

/// `PATCH /api/audiobooks/{id}`: fields you send replace what the scan found
/// (an empty string clears one), and the book is then left alone by rescans.
pub async fn edit_book(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
    Json(e): Json<MetaEdit>,
) -> ApiResult<Json<Book>> {
    let current = get_book(&s.pool, id, uid).await?;
    let blank = |v: Option<String>, old: Option<String>| match v {
        Some(t) => {
            let t = t.trim().to_string();
            (!t.is_empty()).then_some(t)
        }
        None => old,
    };
    let title = match e.title.map(|t| t.trim().to_string()) {
        Some(t) if t.is_empty() => {
            return Err(MusicError::BadRequest("a book needs a title".into()).into())
        }
        Some(t) => t,
        None => current.title.clone(),
    };
    sqlx::query(
        "UPDATE audiobooks SET title = ?, author = ?, narrator = ?, series = ?, series_index = ?,
           year = ?, meta_edited = 1 WHERE id = ?",
    )
    .bind(title)
    .bind(blank(e.author, current.author))
    .bind(blank(e.narrator, current.narrator))
    .bind(blank(e.series, current.series))
    .bind(e.series_index.or(current.series_index))
    .bind(e.year.or(current.year))
    .bind(id)
    .execute(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(Json(get_book(&s.pool, id, uid).await?))
}

// --- position, sessions, finishing -----------------------------------------

#[derive(Deserialize)]
pub struct PositionBody {
    pub book_offset_ms: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PositionOut {
    pub book_offset_ms: i64,
    pub updated_at: i64,
    pub finished: bool,
}

pub async fn put_position(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
    Json(b): Json<PositionBody>,
) -> ApiResult<Json<PositionOut>> {
    Ok(Json(
        save_position(&s.pool, id, b.book_offset_ms, now_ms(), uid).await?,
    ))
}

/// Save a position and keep the listening sessions and the finished flag in
/// step. `now` is a parameter so the 15-minute and midnight rules can be
/// tested without waiting.
pub async fn save_position(
    pool: &SqlitePool,
    id: i64,
    offset_ms: i64,
    now: i64,
    uid: i64,
) -> Result<PositionOut, MusicError> {
    let mut tx = pool.begin().await.map_err(cvt)?;
    let book = sqlx::query("SELECT duration_ms FROM audiobooks WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("audiobook {id}")))?;
    let duration: i64 = book.get("duration_ms");
    let finished_at: Option<i64> =
        sqlx::query("SELECT finished_at FROM audiobook_finished WHERE book_id = ? AND user_id = ?")
            .bind(id)
            .bind(uid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(cvt)?
            .map(|r| r.get(0));
    let offset = if duration > 0 {
        offset_ms.clamp(0, duration)
    } else {
        offset_ms.max(0)
    };
    let previous: Option<(i64, i64)> =
        sqlx::query(
            "SELECT book_offset_ms, updated_at FROM audiobook_positions WHERE book_id = ? AND user_id = ?",
        )
        .bind(id)
        .bind(uid)
        .fetch_optional(&mut *tx)
            .await
            .map_err(cvt)?
            .map(|r| (r.get(0), r.get(1)));
    sqlx::query(
        "INSERT INTO audiobook_positions (book_id, user_id, book_offset_ms, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(book_id, user_id) DO UPDATE SET book_offset_ms = excluded.book_offset_ms,
                                                     updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(uid)
    .bind(offset)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(cvt)?;

    // Sessions. The latest session ends at the last update; a gap over 15
    // minutes or a new day closes it and opens another. An update that does
    // not move the position after a long silence is the app closing, not
    // listening, and opens nothing.
    let last = sqlx::query(
        "SELECT id, ended_at FROM audiobook_sessions WHERE book_id = ? AND user_id = ? ORDER BY ended_at DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .bind(uid)
    .fetch_optional(&mut *tx)
    .await
    .map_err(cvt)?
    .map(|r| (r.get::<i64, _>(0), r.get::<i64, _>(1)));
    match last {
        Some((sid, ended)) if !ab::starts_new_session(ended, now) => {
            sqlx::query(
                "UPDATE audiobook_sessions SET ended_at = ?, end_offset_ms = ? WHERE id = ?",
            )
            .bind(now)
            .bind(offset)
            .bind(sid)
            .execute(&mut *tx)
            .await
            .map_err(cvt)?;
        }
        _ => {
            let moved = previous.map(|(o, _)| o != offset).unwrap_or(true);
            if moved {
                let start = previous.map(|(o, _)| o).unwrap_or(offset);
                sqlx::query(
                    "INSERT INTO audiobook_sessions (book_id, user_id, started_at, ended_at, start_offset_ms, end_offset_ms)
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(id)
                .bind(uid)
                .bind(now)
                .bind(now)
                .bind(start)
                .bind(offset)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
            }
        }
    }

    // Finished at 97%; rewinding below that by hand clears it.
    let finished = ab::is_finished(offset, duration);
    match (finished, finished_at) {
        (true, None) => {
            sqlx::query(
                "INSERT OR IGNORE INTO audiobook_finished (book_id, user_id, finished_at) VALUES (?, ?, ?)",
            )
                .bind(id)
                .bind(uid)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
        }
        (false, Some(_)) => {
            sqlx::query("DELETE FROM audiobook_finished WHERE book_id = ? AND user_id = ?")
                .bind(id)
                .bind(uid)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
        }
        _ => {}
    }
    tx.commit().await.map_err(cvt)?;
    Ok(PositionOut {
        book_offset_ms: offset,
        updated_at: now,
        finished,
    })
}

#[derive(Deserialize, Default)]
pub struct FinishedBody {
    #[serde(default = "yes")]
    pub finished: bool,
}
fn yes() -> bool {
    true
}

/// `POST /api/audiobooks/{id}/finished`: mark a book finished (or, with
/// `{"finished": false}`, not). Marking finished does not move the position.
pub async fn mark_finished(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
    body: Option<Json<FinishedBody>>,
) -> ApiResult<Json<Book>> {
    let finished = body.map(|b| b.0.finished).unwrap_or(true);
    get_book(&s.pool, id, uid).await?;
    if finished {
        sqlx::query(
            "INSERT OR IGNORE INTO audiobook_finished (book_id, user_id, finished_at) VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(uid)
        .bind(now_ms())
            .execute(&s.pool)
            .await
            .map_err(cvt)?;
    } else {
        sqlx::query("DELETE FROM audiobook_finished WHERE book_id = ? AND user_id = ?")
            .bind(id)
            .bind(uid)
            .execute(&s.pool)
            .await
            .map_err(cvt)?;
    }
    Ok(Json(get_book(&s.pool, id, uid).await?))
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Session {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub start_offset_ms: i64,
    pub end_offset_ms: i64,
    /// Book time covered, `end - start` (0 if you went backwards).
    pub listened_ms: i64,
}

pub async fn history(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<Session>>> {
    get_book(&s.pool, id, uid).await?;
    Ok(Json(
        sqlx::query(
            "SELECT id, started_at, ended_at, start_offset_ms, end_offset_ms FROM audiobook_sessions
             WHERE book_id = ? AND user_id = ? ORDER BY started_at DESC, id DESC",
        )
        .bind(id)
        .bind(uid)
        .fetch_all(&s.pool)
        .await
        .map_err(cvt)?
        .iter()
        .map(|r| {
            let (a, b): (i64, i64) = (r.get("start_offset_ms"), r.get("end_offset_ms"));
            Session {
                id: r.get("id"),
                started_at: r.get("started_at"),
                ended_at: r.get("ended_at"),
                start_offset_ms: a,
                end_offset_ms: b,
                listened_ms: (b - a).max(0),
            }
        })
        .collect(),
    ))
}

// --- bookmarks --------------------------------------------------------------

#[derive(Deserialize)]
pub struct NewBookmark {
    pub book_offset_ms: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// "1:02:03" or "4:05" for a book offset.
pub fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

pub async fn list_bookmarks(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<Bookmark>>> {
    get_book(&s.pool, id, uid).await?;
    Ok(Json(bookmarks_of(&s.pool, id, uid).await?))
}

pub async fn add_bookmark(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
    Json(b): Json<NewBookmark>,
) -> ApiResult<Response> {
    let book = get_book(&s.pool, id, uid).await?;
    let offset = b.book_offset_ms.clamp(0, book.duration_ms.max(0));
    let name = b
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Bookmark at {}", clock(offset)));
    let row = sqlx::query(
        "INSERT INTO audiobook_bookmarks (book_id, user_id, book_offset_ms, name, note, created_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id, book_id, book_offset_ms, name, note, created_at",
    )
    .bind(id)
    .bind(uid)
    .bind(offset)
    .bind(name)
    .bind(b.note.unwrap_or_default())
    .bind(now_ms())
    .fetch_one(&s.pool)
    .await
    .map_err(cvt)?;
    Ok((StatusCode::CREATED, Json(bookmark_from_row(&row))).into_response())
}

#[derive(Deserialize)]
pub struct BookmarkEdit {
    pub name: Option<String>,
    pub note: Option<String>,
}

pub async fn edit_bookmark(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path((id, bid)): Path<(i64, i64)>,
    Json(e): Json<BookmarkEdit>,
) -> ApiResult<Json<Bookmark>> {
    let row = sqlx::query(
        "UPDATE audiobook_bookmarks SET name = COALESCE(?, name), note = COALESCE(?, note)
         WHERE id = ? AND book_id = ? AND user_id = ? RETURNING id, book_id, book_offset_ms, name, note, created_at",
    )
    .bind(e.name.map(|n| n.trim().to_string()))
    .bind(e.note)
    .bind(bid)
    .bind(id)
    .bind(uid)
    .fetch_optional(&s.pool)
    .await
    .map_err(cvt)?
    .ok_or_else(|| MusicError::NotFound(format!("bookmark {bid}")))?;
    Ok(Json(bookmark_from_row(&row)))
}

pub async fn delete_bookmark(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path((id, bid)): Path<(i64, i64)>,
) -> ApiResult<StatusCode> {
    let n =
        sqlx::query("DELETE FROM audiobook_bookmarks WHERE id = ? AND book_id = ? AND user_id = ?")
            .bind(bid)
            .bind(id)
            .bind(uid)
            .execute(&s.pool)
            .await
            .map_err(cvt)?
            .rows_affected();
    if n == 0 {
        return Err(MusicError::NotFound(format!("bookmark {bid}")).into());
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- settings ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct SettingsBody {
    pub speed: Option<f64>,
    pub skip_back_s: Option<i64>,
    pub skip_forward_s: Option<i64>,
}

/// Slowest and fastest playback speed the time-stretch stage accepts.
pub const SPEED_RANGE: (f64, f64) = (0.5, 3.0);

pub async fn put_settings(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
    Json(b): Json<SettingsBody>,
) -> ApiResult<Json<Settings>> {
    get_book(&s.pool, id, uid).await?;
    let cur = get_settings(&s.pool, id, uid).await?;
    if let Some(sp) = b.speed {
        if !sp.is_finite() || sp < SPEED_RANGE.0 || sp > SPEED_RANGE.1 {
            return Err(MusicError::BadRequest(format!(
                "speed must be {} to {}",
                SPEED_RANGE.0, SPEED_RANGE.1
            ))
            .into());
        }
    }
    for (name, v) in [
        ("skip_back_s", b.skip_back_s),
        ("skip_forward_s", b.skip_forward_s),
    ] {
        if v.is_some_and(|v| !(1..=600).contains(&v)) {
            return Err(MusicError::BadRequest(format!("{name} must be 1 to 600")).into());
        }
    }
    let next = Settings {
        speed: b.speed.unwrap_or(cur.speed),
        skip_back_s: b.skip_back_s.unwrap_or(cur.skip_back_s),
        skip_forward_s: b.skip_forward_s.unwrap_or(cur.skip_forward_s),
    };
    sqlx::query(
        "INSERT INTO audiobook_settings (book_id, user_id, speed, skip_back_s, skip_forward_s) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(book_id, user_id) DO UPDATE SET speed = excluded.speed, skip_back_s = excluded.skip_back_s,
                                            skip_forward_s = excluded.skip_forward_s",
    )
    .bind(id)
    .bind(uid)
    .bind(next.speed)
    .bind(next.skip_back_s)
    .bind(next.skip_forward_s)
    .execute(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(Json(next))
}

// --- offset resolver --------------------------------------------------------

#[derive(Deserialize)]
pub struct ResolveQuery {
    pub offset_ms: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Resolved {
    pub track_id: i64,
    pub track_offset_ms: i64,
    pub part_index: i64,
    /// The chapter that contains the offset, if the book has any.
    pub chapter_id: Option<i64>,
}

/// `GET /api/audiobooks/{id}/resolve?offset_ms=N`: which file, and where in
/// it, a book offset falls.
pub async fn resolve(
    State(s): State<AppState>,
    Listener(uid): Listener,
    Path(id): Path<i64>,
    Query(q): Query<ResolveQuery>,
) -> ApiResult<Json<Resolved>> {
    get_book(&s.pool, id, uid).await?;
    let parts = parts_of(&s.pool, id).await?;
    let spans: Vec<ab::PartSpan> = parts
        .iter()
        .map(|p| ab::PartSpan {
            track_id: p.track_id,
            start_offset_ms: p.start_offset_ms,
            duration_ms: p.duration_ms,
        })
        .collect();
    let (track_id, track_offset_ms) = ab::resolve_offset(&spans, q.offset_ms)
        .ok_or_else(|| MusicError::NotFound(format!("audiobook {id} has no parts")))?;
    let part_index = parts
        .iter()
        .find(|p| p.track_id == track_id)
        .map(|p| p.part_index)
        .unwrap_or(0);
    let offset = q.offset_ms.max(0);
    let chapter_id: Option<i64> = sqlx::query(
        "SELECT id FROM audiobook_chapters WHERE book_id = ? AND start_offset_ms <= ?
         ORDER BY start_offset_ms DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .bind(offset)
    .fetch_optional(&s.pool)
    .await
    .map_err(cvt)?
    .map(|r| r.get(0));
    Ok(Json(Resolved {
        track_id,
        track_offset_ms,
        part_index,
        chapter_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::fixtures::{make_track, TrackSpec};
    use axum::{
        body::{to_bytes, Body},
        http::{Method, Request},
        Router,
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    /// One 1-second mp3 with the given tags. `album` is the book title tag.
    fn mp3(
        lib: &std::path::Path,
        dir: &str,
        file: &str,
        title: &str,
        album: &str,
        artist: &str,
        track_no: u32,
    ) {
        make_track(
            lib,
            None,
            &TrackSpec {
                dir,
                file,
                codec: "libmp3lame",
                title,
                artist,
                album,
                album_artist: None,
                track_no,
                year: None,
                genre: Some("Audiobook"),
                art: false,
            },
        );
    }

    struct Env {
        app: Router,
        state: AppState,
        dir: tempfile::TempDir,
        books: std::path::PathBuf,
    }

    async fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let books = dir.path().join("books");
        std::fs::create_dir_all(&books).unwrap();
        let pool = crate::db::open(&dir.path().join("t.db")).await.unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let state = AppState {
            pool,
            jobs: crate::jobs::JobStore::new(),
            config: Arc::new(std::sync::RwLock::new(kahawai_core::ServerConfig {
                music_dirs: vec![music],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: crate::transcode_cache::TranscodeCache::disabled(),
        };
        Env {
            app: crate::app(state.clone()),
            state,
            dir,
            books,
        }
    }

    async fn call(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        call_as(app, None, method, uri, body).await
    }

    async fn call_as(
        app: &Router,
        listener: Option<&str>,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(l) = listener {
            req = req.header(LISTENER_HEADER, l);
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    /// A three-part book split over two disc folders, plus a one-file book.
    async fn library(e: &Env) {
        let b = &e.books;
        let book = "Jane Author/Saga/Vol 2 - 1999 - Folder Title {Folder Narrator}";
        mp3(
            b,
            &format!("{book}/CD 1"),
            "02.mp3",
            "Part two",
            "Tagged Title",
            "Tagged Author",
            2,
        );
        mp3(
            b,
            &format!("{book}/CD 1"),
            "01.mp3",
            "Part one",
            "Tagged Title",
            "Tagged Author",
            1,
        );
        mp3(
            b,
            &format!("{book}/CD 2"),
            "01.mp3",
            "Part three",
            "Tagged Title",
            "Tagged Author",
            1,
        );
        mp3(
            b,
            "Solo Writer/Short Story",
            "story.mp3",
            "Short Story",
            "Short Story",
            "Solo Writer",
            1,
        );
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-roots",
            Some(serde_json::json!({"path": b.to_str().unwrap(), "name": "Books"})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        // The add starts a background scan; run one inline so the test is deterministic.
        let _guard = e.state.scan_lock.lock().await;
        ab::scan(&e.state.pool).await.unwrap();
    }

    #[tokio::test]
    async fn folders_become_books_with_ordered_parts_and_merged_discs() {
        let e = env().await;
        library(&e).await;
        let (st, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list.as_array().unwrap().len(), 2, "two books: {list}");
        let saga = list
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["series"] == "Saga")
            .unwrap();
        // Tags win for title and author; the folder fills narrator, series, year.
        assert_eq!(saga["title"], "Tagged Title");
        assert_eq!(saga["author"], "Tagged Author");
        assert_eq!(saga["narrator"], "Folder Narrator");
        assert_eq!(saga["series_index"], 2.0);
        assert_eq!(saga["year"], 1999);
        let id = saga["id"].as_i64().unwrap();
        let (_, d) = call(&e.app, Method::GET, &format!("/api/audiobooks/{id}"), None).await;
        let parts = d["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3, "CD 1 and CD 2 merge into one book");
        let titles: Vec<_> = parts.iter().map(|p| p["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Part one", "Part two", "Part three"]);
        // start_offset = sum of earlier durations.
        let mut at = 0;
        for p in parts {
            assert_eq!(p["start_offset_ms"], at);
            at += p["duration_ms"].as_i64().unwrap();
        }
        assert_eq!(d["duration_ms"], at);
        assert!(at > 2500, "three one-second parts: {at}");
        // No embedded chapters: one chapter per part.
        assert_eq!(d["chapters"].as_array().unwrap().len(), 3);
        assert_eq!(d["settings"]["speed"], 1.0);
    }

    #[tokio::test]
    async fn a_rescan_keeps_the_book_and_what_hangs_on_it() {
        let e = env().await;
        library(&e).await;
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        let id = list[0]["id"].as_i64().unwrap();
        call(
            &e.app,
            Method::PUT,
            &format!("/api/audiobooks/{id}/position"),
            Some(serde_json::json!({"book_offset_ms": 800})),
        )
        .await;
        call(
            &e.app,
            Method::POST,
            &format!("/api/audiobooks/{id}/bookmarks"),
            Some(serde_json::json!({"book_offset_ms": 500, "name": "Mark"})),
        )
        .await;
        ab::scan(&e.state.pool).await.unwrap();
        let (_, list2) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(list2.as_array().unwrap().len(), 2);
        let (_, d) = call(&e.app, Method::GET, &format!("/api/audiobooks/{id}"), None).await;
        assert_eq!(d["position_ms"], 800);
        assert_eq!(d["bookmarks"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn audiobook_files_stream_and_stay_out_of_the_music_catalog() {
        let e = env().await;
        library(&e).await;
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        let id = list[0]["id"].as_i64().unwrap();
        let (_, d) = call(&e.app, Method::GET, &format!("/api/audiobooks/{id}"), None).await;
        let track = d["parts"][0]["track_id"].as_i64().unwrap();
        let res = e
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/stream/{track}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "audiobook roots count for /stream's root check"
        );
        // Not in the music catalog, search, genres, or the live counts.
        let snap = crate::catalog::snapshot(&e.state.pool).await.unwrap();
        assert!(
            snap.tracks.is_empty() && snap.albums.is_empty(),
            "music catalog has no audiobook tracks"
        );
        let n: i64 = sqlx::query("SELECT COUNT(*) FROM search_fts")
            .fetch_one(&e.state.pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(n, 0);
        let (_, genres) = call(&e.app, Method::GET, "/api/genres", None).await;
        assert!(
            genres.as_array().map(|g| g.is_empty()).unwrap_or(true),
            "{genres}"
        );
    }

    #[tokio::test]
    async fn the_music_scan_leaves_audiobook_folders_and_rows_alone() {
        let e = env().await;
        library(&e).await;
        // An audiobook folder that sits inside a music folder, and a real album.
        let music = e.state.music_dirs()[0].clone();
        make_track(
            &music,
            None,
            &TrackSpec {
                dir: "Artist/Album",
                file: "01.mp3",
                codec: "libmp3lame",
                title: "Song",
                artist: "Artist",
                album: "Album",
                album_artist: None,
                track_no: 1,
                year: None,
                genre: None,
                art: false,
            },
        );
        let nested = music.join("Books In Music");
        std::fs::create_dir_all(&nested).unwrap();
        mp3(
            &nested,
            "Nested Author/Nested Book",
            "a.mp3",
            "Nested",
            "Nested Book",
            "Nested Author",
            1,
        );
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-roots",
            Some(serde_json::json!({"path": nested.to_str().unwrap()})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        // Adding a folder starts a scan in the background; let it finish.
        drop(e.state.scan_lock.lock().await);
        ab::scan(&e.state.pool).await.unwrap();

        let report =
            crate::scanner::run_scan_with_progress(&e.state.pool, &e.state.music_dirs(), |_, _| {})
                .await
                .unwrap();
        assert_eq!(report.files_added, 1, "only the album track is music");
        let books_missing: i64 =
            sqlx::query("SELECT COUNT(*) FROM tracks WHERE kind = 'audiobook' AND missing = 1")
                .fetch_one(&e.state.pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(
            books_missing, 0,
            "a music scan never marks audiobook files missing"
        );
        let music_tracks: i64 = sqlx::query("SELECT COUNT(*) FROM tracks WHERE kind = 'music'")
            .fetch_one(&e.state.pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(music_tracks, 1);
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(list.as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn roots_are_validated_and_deleting_one_removes_its_books() {
        let e = env().await;
        library(&e).await;
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-roots",
            Some(serde_json::json!({"path": "/no/such/folder"})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-roots",
            Some(serde_json::json!({"path": e.books.to_str().unwrap()})),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "already registered");
        let inner = e.books.join("Solo Writer");
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-roots",
            Some(serde_json::json!({"path": inner.to_str().unwrap()})),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "inside an existing root");
        let (_, roots) = call(&e.app, Method::GET, "/api/audiobook-roots", None).await;
        let rid = roots[0]["id"].as_i64().unwrap();
        assert_eq!(roots[0]["name"], "Books");
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/audiobook-roots/{rid}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert!(list.as_array().unwrap().is_empty());
        let n: i64 = sqlx::query("SELECT COUNT(*) FROM tracks WHERE kind = 'audiobook'")
            .fetch_one(&e.state.pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(n, 0, "track rows go with the folder");
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/audiobook-roots/{rid}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(
            e.books.join("Solo Writer").exists(),
            "the files stay on disk"
        );
    }

    #[tokio::test]
    async fn an_unmounted_folder_does_not_look_like_a_deleted_library() {
        let e = env().await;
        library(&e).await;
        let moved = e.dir.path().join("books-away");
        std::fs::rename(&e.books, &moved).unwrap();
        let r = ab::scan(&e.state.pool).await.unwrap();
        assert_eq!(r.unreadable_roots, 1);
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(list.as_array().unwrap().len(), 2, "books stay listed");
        // A book whose files really went away stops being listed, but keeps its rows.
        std::fs::rename(&moved, &e.books).unwrap();
        std::fs::remove_dir_all(e.books.join("Solo Writer")).unwrap();
        ab::scan(&e.state.pool).await.unwrap();
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        let n: i64 = sqlx::query("SELECT COUNT(*) FROM audiobooks")
            .fetch_one(&e.state.pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(n, 2, "position and bookmarks would survive its return");
    }

    #[tokio::test]
    async fn positions_sessions_and_finishing() {
        let e = env().await;
        library(&e).await;
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        let id = list[0]["id"].as_i64().unwrap();
        let dur = list[0]["duration_ms"].as_i64().unwrap();
        let t0 = 1_800_000_000_000i64 - (1_800_000_000_000i64 % 86_400_000) + 10 * 3_600_000;
        let pool = &e.state.pool;

        // 10 s apart: one session. 30 min later: a second.
        save_position(pool, id, 100, t0, 0).await.unwrap();
        save_position(pool, id, 400, t0 + 10_000, 0).await.unwrap();
        save_position(pool, id, 700, t0 + 20_000, 0).await.unwrap();
        let later = t0 + 30 * 60_000;
        save_position(pool, id, 900, later, 0).await.unwrap();
        let (_, h) = call(
            &e.app,
            Method::GET,
            &format!("/api/audiobooks/{id}/history"),
            None,
        )
        .await;
        let h = h.as_array().unwrap();
        assert_eq!(h.len(), 2, "{h:?}");
        assert_eq!(h[0]["end_offset_ms"], 900, "newest first");
        assert_eq!(h[1]["start_offset_ms"], 100);
        assert_eq!(h[1]["end_offset_ms"], 700);
        assert_eq!(h[1]["listened_ms"], 600);

        // The app closing later at the same spot opens no empty session.
        save_position(pool, id, 900, later + 3 * 3_600_000, 0)
            .await
            .unwrap();
        let (_, h) = call(
            &e.app,
            Method::GET,
            &format!("/api/audiobooks/{id}/history"),
            None,
        )
        .await;
        assert_eq!(h.as_array().unwrap().len(), 2);

        // Across midnight, even a minute apart, is a new session.
        let before_midnight = t0 - 10 * 3_600_000 + 86_400_000 - 30_000 + 86_400_000;
        save_position(pool, id, 1000, before_midnight, 0)
            .await
            .unwrap();
        save_position(pool, id, 1100, before_midnight + 60_000, 0)
            .await
            .unwrap();
        let (_, h) = call(
            &e.app,
            Method::GET,
            &format!("/api/audiobooks/{id}/history"),
            None,
        )
        .await;
        assert_eq!(h.as_array().unwrap().len(), 4, "the day-change rule: {h:?}");

        // Clamped to the book, and 97% finishes it.
        let out = save_position(pool, id, dur + 99_999, before_midnight + 120_000, 0)
            .await
            .unwrap();
        assert_eq!(out.book_offset_ms, dur);
        assert!(out.finished);
        let (_, b) = call(&e.app, Method::GET, &format!("/api/audiobooks/{id}"), None).await;
        assert!(b["finished_at"].is_i64());
        // Rewinding clears it.
        save_position(pool, id, 10, before_midnight + 130_000, 0)
            .await
            .unwrap();
        let (_, b) = call(&e.app, Method::GET, &format!("/api/audiobooks/{id}"), None).await;
        assert!(b["finished_at"].is_null());
        // The explicit endpoint, both ways.
        let (_, b) = call(
            &e.app,
            Method::POST,
            &format!("/api/audiobooks/{id}/finished"),
            None,
        )
        .await;
        assert!(b["finished_at"].is_i64());
        let (_, b) = call(
            &e.app,
            Method::POST,
            &format!("/api/audiobooks/{id}/finished"),
            Some(serde_json::json!({"finished": false})),
        )
        .await;
        assert!(b["finished_at"].is_null());
        let (st, _) = call(
            &e.app,
            Method::PUT,
            "/api/audiobooks/9999/position",
            Some(serde_json::json!({"book_offset_ms": 1})),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_continue_shelf_orders_by_recent_play_with_finished_last() {
        let e = env().await;
        library(&e).await;
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        let ids: Vec<i64> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_i64().unwrap())
            .collect();
        let (_, shelf) = call(&e.app, Method::GET, "/api/audiobooks?shelf=continue", None).await;
        assert!(shelf.as_array().unwrap().is_empty(), "nothing started");
        let t = 1_800_000_000_000i64;
        save_position(&e.state.pool, ids[0], 500, t, 0)
            .await
            .unwrap();
        save_position(&e.state.pool, ids[1], 300, t + 60_000, 0)
            .await
            .unwrap();
        let (_, shelf) = call(&e.app, Method::GET, "/api/audiobooks?shelf=continue", None).await;
        let order: Vec<i64> = shelf
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_i64().unwrap())
            .collect();
        assert_eq!(order, [ids[1], ids[0]], "most recent first");
        assert!(shelf[0]["progress"].as_f64().unwrap() > 0.0);
        call(
            &e.app,
            Method::POST,
            &format!("/api/audiobooks/{}/finished", ids[1]),
            None,
        )
        .await;
        let (_, shelf) = call(&e.app, Method::GET, "/api/audiobooks?shelf=continue", None).await;
        let order: Vec<i64> = shelf
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_i64().unwrap())
            .collect();
        assert_eq!(order, [ids[0], ids[1]], "unfinished first");
    }

    #[tokio::test]
    async fn bookmarks_settings_resolve_and_manual_edits() {
        let e = env().await;
        library(&e).await;
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks?series=Saga", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        let id = list[0]["id"].as_i64().unwrap();
        let base = format!("/api/audiobooks/{id}");

        let (st, bm) = call(
            &e.app,
            Method::POST,
            &format!("{base}/bookmarks"),
            Some(serde_json::json!({"book_offset_ms": 65_000})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let (_, detail) = call(&e.app, Method::GET, &base, None).await;
        let dur = detail["duration_ms"].as_i64().unwrap();
        assert_eq!(bm["book_offset_ms"], dur, "clamped to the end of the book");
        assert_eq!(
            bm["name"],
            format!("Bookmark at {}", clock(dur)),
            "the default name is the time"
        );
        let bid = bm["id"].as_i64().unwrap();
        let (_, bm2) = call(
            &e.app,
            Method::PATCH,
            &format!("{base}/bookmarks/{bid}"),
            Some(serde_json::json!({"name": "Renamed", "note": "n"})),
        )
        .await;
        assert_eq!(
            (bm2["name"].as_str(), bm2["note"].as_str()),
            (Some("Renamed"), Some("n"))
        );
        let (_, all) = call(&e.app, Method::GET, &format!("{base}/bookmarks"), None).await;
        assert_eq!(all.as_array().unwrap().len(), 1);
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("{base}/bookmarks/{bid}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("{base}/bookmarks/{bid}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        let (st, s) = call(
            &e.app,
            Method::PUT,
            &format!("{base}/settings"),
            Some(serde_json::json!({"speed": 1.5, "skip_back_s": 20})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            (
                s["speed"].as_f64(),
                s["skip_back_s"].as_i64(),
                s["skip_forward_s"].as_i64()
            ),
            (Some(1.5), Some(20), Some(30))
        );
        let (_, d) = call(&e.app, Method::GET, &base, None).await;
        assert_eq!(d["settings"]["speed"], 1.5, "applied when the book starts");
        for bad in [
            serde_json::json!({"speed": 9.0}),
            serde_json::json!({"speed": 0.1}),
            serde_json::json!({"skip_forward_s": 0}),
        ] {
            let (st, _) = call(&e.app, Method::PUT, &format!("{base}/settings"), Some(bad)).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
        }

        // resolve: offsets map to a file and a place in it.
        let parts = d["parts"].as_array().unwrap();
        let second = &parts[1];
        let start = second["start_offset_ms"].as_i64().unwrap();
        let (_, r) = call(
            &e.app,
            Method::GET,
            &format!("{base}/resolve?offset_ms={}", start + 250),
            None,
        )
        .await;
        assert_eq!(r["track_id"], second["track_id"]);
        assert_eq!(r["track_offset_ms"], 250);
        assert_eq!(r["part_index"], 1);
        assert!(r["chapter_id"].is_i64());
        let (_, r) = call(
            &e.app,
            Method::GET,
            &format!("{base}/resolve?offset_ms=999999999"),
            None,
        )
        .await;
        assert_eq!(
            r["track_id"], parts[2]["track_id"],
            "past the end: the last part"
        );

        // A manual edit sticks across a rescan.
        let (st, b) = call(
            &e.app,
            Method::PATCH,
            &base,
            Some(serde_json::json!({"title": "My Title", "narrator": ""})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(b["title"], "My Title");
        assert!(b["narrator"].is_null(), "an empty string clears a field");
        ab::scan(&e.state.pool).await.unwrap();
        let (_, d) = call(&e.app, Method::GET, &base, None).await;
        assert_eq!(d["title"], "My Title");
        let (st, _) = call(
            &e.app,
            Method::PATCH,
            &base,
            Some(serde_json::json!({"title": " "})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn search_and_filters() {
        let e = env().await;
        library(&e).await;
        let n = |q: &str| {
            let app = e.app.clone();
            let q = q.to_string();
            async move {
                let (_, v) = call(&app, Method::GET, &format!("/api/audiobooks{q}"), None).await;
                v.as_array().unwrap().len()
            }
        };
        assert_eq!(n("?q=tagged").await, 1, "title or author");
        assert_eq!(n("?q=folder%20narrator").await, 1, "narrator");
        assert_eq!(n("?q=zzz").await, 0);
        assert_eq!(n("?q=%25").await, 0, "a % is not a wildcard");
        assert_eq!(n("?author=Solo%20Writer").await, 1);
        assert_eq!(n("?finished=true").await, 0);
        assert_eq!(n("?finished=false").await, 2);
    }

    #[tokio::test]
    async fn online_lookup_is_opt_in_and_checks_the_book() {
        let e = env().await;
        library(&e).await;
        let (st, v) = call(&e.app, Method::POST, "/api/audiobooks/enrich", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("online lookup is off"),
            "{v}"
        );
        e.state.config.write().unwrap().enrichment_enabled = true;
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobooks/enrich",
            Some(serde_json::json!({"book_id": 9999})),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::NOT_FOUND,
            "an unknown book is refused before anything goes online"
        );
    }

    #[tokio::test]
    async fn a_rescan_keeps_what_a_lookup_filled_in() {
        let e = env().await;
        library(&e).await;
        let (_, list) = call(
            &e.app,
            Method::GET,
            "/api/audiobooks?author=Solo%20Writer",
            None,
        )
        .await;
        let id = list[0]["id"].as_i64().unwrap();
        assert!(
            list[0]["year"].is_null(),
            "the folder and tags give no year"
        );
        sqlx::query("UPDATE audiobooks SET year = 1984, cover_hash = 'abc' WHERE id = ?")
            .bind(id)
            .execute(&e.state.pool)
            .await
            .unwrap();
        ab::scan(&e.state.pool).await.unwrap();
        let (_, d) = call(&e.app, Method::GET, &format!("/api/audiobooks/{id}"), None).await;
        assert_eq!(
            (d["year"].as_i64(), d["cover_hash"].as_str()),
            (Some(1984), Some("abc"))
        );
    }

    #[tokio::test]
    async fn a_scan_reports_files_as_it_reads_them_with_an_estimate_from_the_last_one() {
        let e = env().await;
        library(&e).await;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u64, Option<u64>)>::new()));
        // library() scanned once already, so this is a rescan: 4 files known.
        let sink = seen.clone();
        ab::scan_with_progress(
            &e.state.pool,
            std::sync::Arc::new(move |d, est| sink.lock().unwrap().push((d, est))),
        )
        .await
        .unwrap();
        let v = seen.lock().unwrap().clone();
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(v.iter().all(|x| x.1 == Some(4)), "{v:?}");
        // A first scan has no estimate.
        let fresh = env().await;
        library_files(&fresh);
        sqlx::query("INSERT INTO audiobook_roots (path, name) VALUES (?, 'b')")
            .bind(fresh.books.to_str().unwrap())
            .execute(&fresh.state.pool)
            .await
            .unwrap();
        let first = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Option<u64>>::new()));
        let sink = first.clone();
        ab::scan_with_progress(
            &fresh.state.pool,
            std::sync::Arc::new(move |_, est| sink.lock().unwrap().push(est)),
        )
        .await
        .unwrap();
        assert!(first.lock().unwrap().iter().all(|e| e.is_none()));
    }

    /// The library's files without registering the folder.
    fn library_files(e: &Env) {
        let b = &e.books;
        mp3(b, "A/One", "01.mp3", "One", "One", "A", 1);
        mp3(b, "A/Two", "01.mp3", "Two", "Two", "A", 1);
    }

    /// Copy one book's folder to another place under the same root.
    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dest = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &dest);
            } else {
                std::fs::copy(e.path(), dest).unwrap();
            }
        }
    }

    async fn hashed_files(e: &Env) -> i64 {
        sqlx::query("SELECT COUNT(*) FROM tracks WHERE kind = 'audiobook' AND hash IS NOT NULL")
            .fetch_one(&e.state.pool)
            .await
            .unwrap()
            .get(0)
    }

    #[tokio::test]
    async fn a_copy_of_a_book_is_hidden_and_a_file_of_unique_size_is_never_hashed() {
        let e = env().await;
        // A book whose one file has a size no other file has.
        let odd = e.books.join("Odd Author/Odd Book");
        std::fs::create_dir_all(&odd).unwrap();
        library_files_into(&e.books);
        let mut bytes = std::fs::read(e.books.join("A/One/01.mp3")).unwrap();
        bytes.extend_from_slice(&[0u8; 17]);
        std::fs::write(odd.join("odd.mp3"), bytes).unwrap();
        sqlx::query("INSERT INTO audiobook_roots (path, name) VALUES (?, 'b')")
            .bind(e.books.to_str().unwrap())
            .execute(&e.state.pool)
            .await
            .unwrap();
        ab::scan(&e.state.pool).await.unwrap();
        let odd_hash: Option<String> =
            sqlx::query("SELECT hash FROM tracks WHERE path LIKE '%odd.mp3'")
                .fetch_one(&e.state.pool)
                .await
                .unwrap()
                .get(0);
        assert!(
            odd_hash.is_none(),
            "nothing else is that size: no reason to read it all"
        );

        // A backup copy of a book, in another folder.
        copy_dir(&e.books.join("A/One"), &e.books.join("Backup/One copy"));
        let (_, before) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        let listed_before = before.as_array().unwrap().len();
        let r = ab::scan(&e.state.pool).await.unwrap();
        assert_eq!(r.duplicates, 1, "{r:?}");
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(
            list.as_array().unwrap().len(),
            listed_before,
            "the copy was not added to the library: {list}"
        );
        let dup: i64 =
            sqlx::query("SELECT COUNT(*) FROM audiobooks WHERE duplicate_of IS NOT NULL")
                .fetch_one(&e.state.pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(dup, 1);
        // Idempotent: another scan changes nothing.
        let hashed = hashed_files(&e).await;
        assert_eq!(ab::scan(&e.state.pool).await.unwrap().duplicates, 1);
        assert_eq!(hashed_files(&e).await, hashed);
    }

    /// Two small books, `A/One` and `A/Two` (identical audio, so identical sizes).
    fn library_files_into(root: &std::path::Path) {
        mp3(root, "A/One", "01.mp3", "One", "One", "A", 1);
        mp3(root, "A/Two", "01.mp3", "Two", "Two", "A", 1);
    }

    #[tokio::test]
    async fn the_copy_with_listening_progress_is_the_one_kept() {
        let e = env().await;
        library(&e).await;
        let src = e
            .books
            .join("Jane Author/Saga/Vol 2 - 1999 - Folder Title {Folder Narrator}");
        copy_dir(&src, &e.books.join("Backup/Folder Title copy"));
        ab::scan(&e.state.pool).await.unwrap();
        // Listen to the one that was hidden.
        let hidden: i64 = sqlx::query("SELECT id FROM audiobooks WHERE duplicate_of IS NOT NULL")
            .fetch_one(&e.state.pool)
            .await
            .unwrap()
            .get(0);
        save_position(&e.state.pool, hidden, 1000, 1_800_000_000_000, 0)
            .await
            .unwrap();
        ab::find_duplicates(&e.state.pool).await.unwrap();
        let now_hidden: i64 =
            sqlx::query("SELECT id FROM audiobooks WHERE duplicate_of IS NOT NULL")
                .fetch_one(&e.state.pool)
                .await
                .unwrap()
                .get(0);
        assert_ne!(
            now_hidden, hidden,
            "the one you were listening to is the one that shows"
        );
    }

    #[tokio::test]
    async fn the_same_size_but_different_content_is_not_a_copy_and_a_removed_copy_unhides() {
        let e = env().await;
        library(&e).await;
        // Same layout and sizes, different audio: a different book.
        let src = e.books.join("Solo Writer/Short Story/story.mp3");
        let dir = e.books.join("Other Writer/Another Story");
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = std::fs::read(&src).unwrap();
        let n = bytes.len();
        bytes[n - 10] ^= 0xFF; // one byte differs, the size does not
        std::fs::write(dir.join("story.mp3"), bytes).unwrap();
        let r = ab::scan(&e.state.pool).await.unwrap();
        assert_eq!(r.duplicates, 0, "same size, different bytes: both stay");
        let differ: i64 =
            sqlx::query("SELECT COUNT(DISTINCT hash) FROM tracks WHERE path LIKE '%story.mp3'")
                .fetch_one(&e.state.pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(differ, 2, "they were hashed, and the hashes differ");
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(list.as_array().unwrap().len(), 3);

        // A true copy, then remove it: the original is unaffected, and nothing stays hidden.
        copy_dir(
            &e.books.join("Solo Writer/Short Story"),
            &e.books.join("Backup/Short Story"),
        );
        assert_eq!(ab::scan(&e.state.pool).await.unwrap().duplicates, 1);
        std::fs::remove_dir_all(e.books.join("Backup")).unwrap();
        let r = ab::scan(&e.state.pool).await.unwrap();
        assert_eq!(r.duplicates, 0);
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        assert_eq!(list.as_array().unwrap().len(), 3);
    }

    /// An m4b with two embedded chapters, made by ffmpeg.
    #[tokio::test]
    async fn m4b_chapters_are_read_from_the_file() {
        let e = env().await;
        let dir = e.books.join("Chapter Author/Chaptered");
        std::fs::create_dir_all(&dir).unwrap();
        let meta = e.dir.path().join("chapters.txt");
        std::fs::write(
            &meta,
            ";FFMETADATA1\ntitle=Chaptered\nartist=Chapter Author\nalbum=Chaptered\n\
             [CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Opening\n\
             [CHAPTER]\nTIMEBASE=1/1000\nSTART=1500\nEND=4000\ntitle=The Middle\n",
        )
        .unwrap();
        let out = dir.join("book.m4b");
        let st = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4:sample_rate=44100",
                "-i",
            ])
            .arg(&meta)
            .args([
                "-map",
                "0:a",
                "-map_metadata",
                "1",
                "-map_chapters",
                "1",
                "-c:a",
                "aac",
                "-f",
                "ipod",
            ])
            .arg(&out)
            .status()
            .expect("ffmpeg is required for the fixtures");
        assert!(st.success());
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-roots",
            Some(serde_json::json!({"path": e.books.to_str().unwrap()})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let _g = e.state.scan_lock.lock().await;
        ab::scan(&e.state.pool).await.unwrap();
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        let id = list[0]["id"].as_i64().unwrap();
        let (_, d) = call(&e.app, Method::GET, &format!("/api/audiobooks/{id}"), None).await;
        assert_eq!(
            d["parts"].as_array().unwrap().len(),
            1,
            "one m4b is one part"
        );
        let ch = d["chapters"].as_array().unwrap();
        assert_eq!(ch.len(), 2, "{ch:?}");
        assert_eq!(ch[0]["title"], "Opening");
        assert_eq!(ch[1]["title"], "The Middle");
        let start = ch[1]["start_offset_ms"].as_i64().unwrap();
        assert!(
            (1400..=1600).contains(&start),
            "second chapter starts near 1.5 s: {start}"
        );
    }

    #[tokio::test]
    async fn listeners_keep_their_own_place_bookmarks_history_and_finished_flag() {
        let e = env().await;
        library(&e).await;
        let (_, list) = call(&e.app, Method::GET, "/api/audiobooks", None).await;
        let id = list[0]["id"].as_i64().unwrap();
        let dur = list[0]["duration_ms"].as_i64().unwrap();
        let book = format!("/api/audiobooks/{id}");

        // Ann (percent-encoded, with a space) listens; the default listener does not.
        let ann = Some("Ann%20B");
        let (st, _) = call_as(
            &e.app,
            ann,
            Method::PUT,
            &format!("{book}/position"),
            Some(serde_json::json!({"book_offset_ms": 500})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        call_as(
            &e.app,
            ann,
            Method::POST,
            &format!("{book}/bookmarks"),
            Some(serde_json::json!({"book_offset_ms": 100})),
        )
        .await;
        call_as(
            &e.app,
            ann,
            Method::PUT,
            &format!("{book}/settings"),
            Some(serde_json::json!({"speed": 1.5})),
        )
        .await;
        call_as(&e.app, ann, Method::POST, &format!("{book}/finished"), None).await;

        let (_, mine) = call_as(&e.app, ann, Method::GET, &book, None).await;
        assert_eq!(mine["position_ms"], 500);
        assert_eq!(mine["bookmarks"].as_array().unwrap().len(), 1);
        assert_eq!(mine["settings"]["speed"], 1.5);
        assert!(!mine["finished_at"].is_null());
        let (_, h) = call_as(&e.app, ann, Method::GET, &format!("{book}/history"), None).await;
        assert_eq!(h.as_array().unwrap().len(), 1);

        let (_, other) = call(&e.app, Method::GET, &book, None).await;
        assert!(other["position_ms"] == 0, "{other}");
        assert!(other["bookmarks"].as_array().unwrap().is_empty());
        assert_eq!(other["settings"]["speed"], 1.0);
        assert!(other["finished_at"].is_null());
        let (_, h) = call(&e.app, Method::GET, &format!("{book}/history"), None).await;
        assert!(h.as_array().unwrap().is_empty());
        let (_, shelf) = call(&e.app, Method::GET, "/api/audiobooks?shelf=continue", None).await;
        assert!(shelf.as_array().unwrap().is_empty());
        let (_, shelf) = call_as(
            &e.app,
            ann,
            Method::GET,
            "/api/audiobooks?shelf=continue",
            None,
        )
        .await;
        assert_eq!(shelf.as_array().unwrap().len(), 1);
        let _ = dur;

        // Listeners were created on first use and are listed.
        let (_, ls) = call(&e.app, Method::GET, "/api/audiobook-listeners", None).await;
        let names: Vec<&str> = ls
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Default", "Ann B"]);
    }

    #[tokio::test]
    async fn listeners_can_be_added_validated_and_removed() {
        let e = env().await;
        let (st, l) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-listeners",
            Some(serde_json::json!({"name": "  Bo  "})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(l["name"], "Bo");
        let uid = l["id"].as_i64().unwrap();
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/audiobook-listeners",
            Some(serde_json::json!({"name": "   "})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = call(&e.app, Method::DELETE, "/api/audiobook-listeners/0", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "the default cannot go");
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/audiobook-listeners/{uid}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (_, ls) = call(&e.app, Method::GET, "/api/audiobook-listeners", None).await;
        assert_eq!(ls.as_array().unwrap().len(), 1);
    }
}
