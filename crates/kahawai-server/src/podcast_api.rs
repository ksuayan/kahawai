//! `/api/podcasts/*`: subscriptions, episodes, refresh and OPML
//! (docs/v2/kahawai-podcast-spec.md, D1 and D2).

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use kahawai_core::MusicError;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::api::ApiError;
use crate::db::cvt;
use crate::podcast_dl;
use crate::podcast_feed;
use crate::podcasts::{self, FeedRow, Refreshed};
use crate::AppState;

type ApiResult<T> = Result<T, ApiError>;

/// Feeds are refreshed this many at a time after an import.
const IMPORT_PARALLEL: usize = 4;

pub async fn list_feeds(State(s): State<AppState>) -> ApiResult<Json<Vec<FeedRow>>> {
    Ok(Json(podcasts::list_feeds(&s.pool).await?))
}

#[derive(Deserialize)]
pub struct NewFeed {
    pub url: String,
}

#[derive(Serialize)]
pub struct Subscribed {
    pub feed: FeedRow,
    pub episodes_added: usize,
    pub warnings: Vec<String>,
}

/// `POST /api/podcasts/feeds {url}`: subscribe. The address is fetched and read
/// first, so a page that is not a feed (or a dead link) is refused with the
/// reason, and nothing is saved.
pub async fn add_feed(State(s): State<AppState>, Json(b): Json<NewFeed>) -> ApiResult<Response> {
    let url = podcast_feed::normalize_feed_url(&b.url).map_err(MusicError::BadRequest)?;
    let exists = sqlx::query("SELECT 1 FROM podcast_feeds WHERE feed_url = ?")
        .bind(&url)
        .fetch_optional(&s.pool)
        .await
        .map_err(cvt)?
        .is_some();
    if exists {
        return Err(MusicError::Conflict("you already subscribe to that podcast".into()).into());
    }
    let (parsed, etag, modified) = podcasts::fetch_and_parse(&url).await?;
    let id = podcasts::insert_feed(&s.pool, &url, &parsed.title).await?;
    let stored =
        podcasts::store(&s.pool, id, &parsed, etag.as_deref(), modified.as_deref()).await?;
    let state = s.clone();
    tokio::spawn(async move {
        let _ = podcast_dl::auto_download(&state, id).await;
    });
    let out = Subscribed {
        feed: podcasts::get_feed(&s.pool, id).await?,
        episodes_added: stored.episodes_added,
        warnings: parsed.warnings,
    };
    Ok((StatusCode::CREATED, Json(out)).into_response())
}

pub async fn delete_feed(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    podcasts::delete_feed(&s.pool, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
pub struct RefreshBody {
    #[serde(default)]
    pub feed_id: Option<i64>,
}

/// `POST /api/podcasts/refresh {feed_id?}`: read one feed again, or all of
/// them. A feed that fails keeps its episodes and records why on itself.
pub async fn refresh(
    State(s): State<AppState>,
    body: Option<Json<RefreshBody>>,
) -> ApiResult<Json<Vec<Refreshed>>> {
    let only = body.and_then(|b| b.0.feed_id);
    let ids: Vec<i64> = match only {
        Some(id) => vec![podcasts::get_feed(&s.pool, id).await?.id],
        None => podcasts::list_feeds(&s.pool)
            .await?
            .iter()
            .map(|f| f.id)
            .collect(),
    };
    let done = refresh_many(&s.pool, ids).await;
    for r in &done {
        if r.error.is_none() && !r.unchanged {
            let (state, id) = (s.clone(), r.feed_id);
            tokio::spawn(async move {
                let _ = podcast_dl::auto_download(&state, id).await;
            });
        }
    }
    Ok(Json(done))
}

async fn refresh_many(pool: &sqlx::SqlitePool, ids: Vec<i64>) -> Vec<Refreshed> {
    use futures_util_lite::join_limited;
    join_limited(ids, IMPORT_PARALLEL, |id| {
        let pool = pool.clone();
        async move {
            podcasts::refresh_feed(&pool, id)
                .await
                .unwrap_or_else(|e| Refreshed {
                    feed_id: id,
                    unchanged: false,
                    episodes_added: 0,
                    warnings: Vec::new(),
                    error: Some(e.to_string()),
                })
        }
    })
    .await
}

/// A tiny bounded-parallel map (no extra dependency): runs `f` over `items`,
/// at most `limit` at a time, results in input order.
mod futures_util_lite {
    use std::future::Future;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    pub async fn join_limited<T, R, F, Fut>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: Fn(T) -> Fut,
        Fut: Future<Output = R> + Send + 'static,
    {
        let sem = Arc::new(Semaphore::new(limit.max(1)));
        let mut handles = Vec::with_capacity(items.len());
        for item in items {
            let permit_sem = sem.clone();
            let fut = f(item);
            handles.push(tokio::spawn(async move {
                let _p = permit_sem.acquire_owned().await.expect("semaphore open");
                fut.await
            }));
        }
        let mut out = Vec::with_capacity(handles.len());
        for h in handles {
            if let Ok(r) = h.await {
                out.push(r);
            }
        }
        out
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Episode {
    pub id: i64,
    pub feed_id: i64,
    pub guid: String,
    pub title: String,
    pub description_html: Option<String>,
    pub published_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub enclosure_url: String,
    pub enclosure_type: Option<String>,
    pub enclosure_bytes: Option<i64>,
    pub image_url: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub link: Option<String>,
    pub downloaded: bool,
    pub played_at: Option<i64>,
    pub dropped_from_feed: bool,
}

#[derive(Deserialize)]
pub struct EpisodeQuery {
    pub unplayed: Option<u8>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// `GET /api/podcasts/feeds/{id}/episodes?unplayed=1&limit=&offset=`, newest first.
pub async fn list_episodes(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<EpisodeQuery>,
) -> ApiResult<Json<Vec<Episode>>> {
    podcasts::get_feed(&s.pool, id).await?;
    let rows = sqlx::query(
        "SELECT id, feed_id, guid, title, description_html, published_at, duration_ms, enclosure_url,
                enclosure_type, enclosure_bytes, image_url, season, episode, link, file_path,
                played_at, dropped_from_feed
         FROM podcast_episodes
         WHERE feed_id = ? AND (? = 0 OR played_at IS NULL)
         ORDER BY published_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(id)
    .bind(i64::from(q.unplayed.unwrap_or(0) != 0))
    .bind(q.limit.unwrap_or(200).clamp(1, 2000))
    .bind(q.offset.unwrap_or(0).max(0))
    .fetch_all(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(Json(
        rows.iter()
            .map(|r| Episode {
                id: r.get("id"),
                feed_id: r.get("feed_id"),
                guid: r.get("guid"),
                title: r.get("title"),
                description_html: r.get("description_html"),
                published_at: r.get("published_at"),
                duration_ms: r.get("duration_ms"),
                enclosure_url: r.get("enclosure_url"),
                enclosure_type: r.get("enclosure_type"),
                enclosure_bytes: r.get("enclosure_bytes"),
                image_url: r.get("image_url"),
                season: r.get("season"),
                episode: r.get("episode"),
                link: r.get("link"),
                downloaded: r.get::<Option<String>, _>("file_path").is_some(),
                played_at: r.get("played_at"),
                dropped_from_feed: r.get::<i64, _>("dropped_from_feed") != 0,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct PlayedBody {
    #[serde(default = "yes")]
    pub played: bool,
}
fn yes() -> bool {
    true
}

/// `POST /api/podcasts/episodes/{id}/played {played}`: mark played or unplayed.
pub async fn mark_played(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<PlayedBody>>,
) -> ApiResult<StatusCode> {
    let played = body.map(|b| b.0.played).unwrap_or(true);
    let n = sqlx::query(
        "UPDATE podcast_episodes SET played_at = CASE WHEN ? = 1 THEN COALESCE(played_at, ?) ELSE NULL END WHERE id = ?",
    )
    .bind(i64::from(played))
    .bind(podcasts::now_ms())
    .bind(id)
    .execute(&s.pool)
    .await
    .map_err(cvt)?
    .rows_affected();
    if n == 0 {
        return Err(MusicError::NotFound(format!("episode {id}")).into());
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- downloads --------------------------------------------------------------

/// `POST /api/podcasts/episodes/{id}/download`: queue the download (a job).
pub async fn download(State(s): State<AppState>, Path(id): Path<i64>) -> ApiResult<Response> {
    let job = podcast_dl::spawn_download(s, id).await?;
    Ok((StatusCode::ACCEPTED, Json(job)).into_response())
}

/// `DELETE /api/podcasts/episodes/{id}/download`: cancel it, or delete the file.
pub async fn delete_download(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    podcast_dl::remove_download(&s, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/podcasts/episodes/{id}/file`: the downloaded audio, with byte ranges.
pub async fn episode_file(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    let row = sqlx::query("SELECT file_path, enclosure_type FROM podcast_episodes WHERE id = ?")
        .bind(id)
        .fetch_optional(&s.pool)
        .await
        .map_err(cvt)?
        .ok_or_else(|| MusicError::NotFound(format!("episode {id}")))?;
    let path: Option<String> = row.get(0);
    let path = path.ok_or_else(|| MusicError::NotFound("that episode is not downloaded".into()))?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| MusicError::NotFound("the downloaded file is missing".into()))?;
    let mime = match podcast_dl::extension_for(row.get::<Option<String>, _>(1).as_deref(), &path) {
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        _ => "audio/mpeg",
    };
    crate::api::serve_ranged(file, &headers, mime).await
}

#[derive(Serialize)]
pub struct FolderInfo {
    pub path: String,
    /// The folder exists (or could be made) and can be written to.
    pub usable: bool,
    pub episodes_downloaded: i64,
    pub bytes_downloaded: u64,
}

/// `GET /api/podcasts/folder`: where downloads go and what is in it.
pub async fn folder(State(s): State<AppState>) -> ApiResult<Json<FolderInfo>> {
    let root = podcast_dl::podcast_root(&s.config.read().unwrap());
    let usable = tokio::fs::create_dir_all(&root).await.is_ok();
    let paths: Vec<String> =
        sqlx::query("SELECT file_path FROM podcast_episodes WHERE file_path IS NOT NULL")
            .fetch_all(&s.pool)
            .await
            .map_err(cvt)?
            .iter()
            .map(|r| r.get(0))
            .collect();
    let mut bytes = 0u64;
    for p in &paths {
        bytes += tokio::fs::metadata(p).await.map(|m| m.len()).unwrap_or(0);
    }
    Ok(Json(FolderInfo {
        path: root.to_string_lossy().to_string(),
        usable,
        episodes_downloaded: paths.len() as i64,
        bytes_downloaded: bytes,
    }))
}

#[derive(Deserialize)]
pub struct FeedSettings {
    pub auto_download: Option<bool>,
    pub keep_n: Option<i64>,
    pub delete_played_after_days: Option<i64>,
}

/// `PUT /api/podcasts/feeds/{id}/settings`: how this feed downloads and tidies.
pub async fn put_feed_settings(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(b): Json<FeedSettings>,
) -> ApiResult<Json<FeedRow>> {
    let cur = podcasts::get_feed(&s.pool, id).await?;
    let keep = b.keep_n.unwrap_or(cur.keep_n);
    let days = b
        .delete_played_after_days
        .unwrap_or(cur.delete_played_after_days);
    if !(1..=100).contains(&keep) {
        return Err(MusicError::BadRequest("keep_n must be 1 to 100".into()).into());
    }
    if !(0..=365).contains(&days) {
        return Err(
            MusicError::BadRequest("delete_played_after_days must be 0 to 365".into()).into(),
        );
    }
    sqlx::query(
        "UPDATE podcast_feeds SET auto_download = ?, keep_n = ?, delete_played_after_days = ? WHERE id = ?",
    )
    .bind(i64::from(b.auto_download.unwrap_or(cur.auto_download)))
    .bind(keep)
    .bind(days)
    .bind(id)
    .execute(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(Json(podcasts::get_feed(&s.pool, id).await?))
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Imported {
    pub added: usize,
    pub already_subscribed: usize,
    pub invalid: Vec<String>,
}

/// `POST /api/podcasts/feeds/import-opml` with the OPML file as the body.
/// Feeds are added at once (named from the file) and read in the background,
/// a few at a time; a feed that turns out broken shows its error on its row.
pub async fn import_opml(State(s): State<AppState>, body: String) -> ApiResult<Json<Imported>> {
    let entries = podcasts::parse_opml(&body);
    if entries.is_empty() {
        return Err(MusicError::BadRequest(
            "no podcasts found in that file (an OPML file lists them as outlines with an xmlUrl)"
                .into(),
        )
        .into());
    }
    let mut out = Imported {
        added: 0,
        already_subscribed: 0,
        invalid: Vec::new(),
    };
    let mut new_ids = Vec::new();
    for e in entries {
        let url = match podcast_feed::normalize_feed_url(&e.url) {
            Ok(u) => u,
            Err(_) => {
                out.invalid.push(e.url);
                continue;
            }
        };
        let title = e.title.unwrap_or_else(|| url.clone());
        match podcasts::insert_feed(&s.pool, &url, &title).await {
            Ok(id) => {
                out.added += 1;
                new_ids.push(id);
            }
            Err(MusicError::Conflict(_)) => out.already_subscribed += 1,
            Err(e) => return Err(e.into()),
        }
    }
    let pool = s.pool.clone();
    tokio::spawn(async move {
        let _ = refresh_many(&pool, new_ids).await;
    });
    Ok(Json(out))
}

/// `GET /api/podcasts/feeds/export-opml`
pub async fn export_opml(State(s): State<AppState>) -> ApiResult<Response> {
    let xml = podcasts::write_opml(&podcasts::list_feeds(&s.pool).await?);
    Ok((
        [
            (header::CONTENT_TYPE, "text/x-opml; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"kahawai-podcasts.opml\"",
            ),
        ],
        xml,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Method, Request},
        routing::get,
        Router,
    };
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    struct Env {
        app: Router,
        state: AppState,
        _dir: tempfile::TempDir,
    }

    async fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("t.db")).await.unwrap();
        let state = AppState {
            pool,
            jobs: crate::jobs::JobStore::new(),
            config: Arc::new(std::sync::RwLock::new(kahawai_core::ServerConfig::default())),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: crate::transcode_cache::TranscodeCache::disabled(),
        };
        Env {
            app: crate::app(state.clone()),
            state,
            _dir: dir,
        }
    }

    async fn call(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let (st, bytes) = call_raw(
            app,
            method,
            uri,
            body.map(|b| ("application/json", b.to_string())),
        )
        .await;
        (
            st,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn call_raw(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<(&str, String)>,
    ) -> (StatusCode, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some((ct, b)) => {
                req = req.header("content-type", ct);
                Body::from(b)
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let st = res.status();
        (
            st,
            to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
    }

    fn feed_xml(title: &str, eps: &[(&str, &str)]) -> String {
        feed_xml_at("https://cdn.example", title, eps)
    }

    fn feed_xml_at(audio_base: &str, title: &str, eps: &[(&str, &str)]) -> String {
        let items: String = eps
            .iter()
            .map(|(g, d)| {
                format!(
                    "<item><title>Episode {g}</title><guid>{g}</guid><pubDate>{d}</pubDate>\
                     <enclosure url=\"{audio_base}/audio/{g}.mp3\" length=\"10\" type=\"audio/mpeg\"/>\
                     <itunes:duration>10:00</itunes:duration></item>"
                )
            })
            .collect();
        format!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\
             <channel><title>{title}</title><link>https://show.example</link>{items}</channel></rss>"
        )
    }

    #[derive(Clone)]
    struct Site {
        feed: Arc<Mutex<String>>,
        etag: Arc<Mutex<Option<String>>>,
        hits: Arc<Mutex<u32>>,
    }

    /// A stand-in feed host: `/feed` (with ETag), `/page` (HTML), `/gone` (404).
    async fn site(xml: String) -> (String, Site) {
        let site = Site {
            feed: Arc::new(Mutex::new(xml)),
            etag: Arc::new(Mutex::new(Some("\"v1\"".into()))),
            hits: Arc::new(Mutex::new(0)),
        };
        let s = site.clone();
        let app = Router::new()
            .route(
                "/feed",
                get(move |headers: axum::http::HeaderMap| {
                    let s = s.clone();
                    async move {
                        *s.hits.lock().unwrap() += 1;
                        let etag = s.etag.lock().unwrap().clone();
                        if let (Some(e), Some(sent)) = (&etag, headers.get("if-none-match")) {
                            if sent.to_str().ok() == Some(e.as_str()) {
                                return (StatusCode::NOT_MODIFIED, [(header::ETAG, e.clone())], String::new()).into_response();
                            }
                        }
                        let body = s.feed.lock().unwrap().clone();
                        match etag {
                            Some(e) => ([(header::ETAG, e), (header::CONTENT_TYPE, "application/rss+xml".to_string())], body).into_response(),
                            None => body.into_response(),
                        }
                    }
                }),
            )
            .route(
                "/page",
                get(|| async { axum::response::Html("<html><head><link rel=\"alternate\" type=\"application/rss+xml\" href=\"/feed\"></head></html>") }),
            )
            .route("/gone", get(|| async { StatusCode::NOT_FOUND }))
            .route(
                "/audio/{name}",
                get(
                    |axum::extract::Path(name): axum::extract::Path<String>,
                     headers: axum::http::HeaderMap| async move {
                        if name.starts_with("html") {
                            return axum::response::Html("<html>login</html>").into_response();
                        }
                        let body: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
                        if let Some(r) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
                            if let Some(from) = r
                                .strip_prefix("bytes=")
                                .and_then(|s| s.strip_suffix('-'))
                                .and_then(|s| s.parse::<usize>().ok())
                            {
                                let part = body[from.min(body.len())..].to_vec();
                                return (
                                    StatusCode::PARTIAL_CONTENT,
                                    [
                                        (header::CONTENT_TYPE, "audio/mpeg".to_string()),
                                        (
                                            header::CONTENT_RANGE,
                                            format!("bytes {from}-{}/{}", body.len() - 1, body.len()),
                                        ),
                                        (header::HeaderName::from_static("x-saw-range"), r.to_string()),
                                    ],
                                    part,
                                )
                                    .into_response();
                            }
                        }
                        ([(header::CONTENT_TYPE, "audio/mpeg")], body).into_response()
                    },
                ),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (base, site)
    }

    const D1: &str = "Mon, 02 Mar 2026 10:00:00 +0000";
    const D2: &str = "Tue, 03 Mar 2026 10:00:00 +0000";
    const D3: &str = "Wed, 04 Mar 2026 10:00:00 +0000";

    #[tokio::test]
    async fn subscribing_reads_the_feed_and_keeps_its_episodes() {
        let e = env().await;
        let (base, _site) = site(feed_xml("Show One", &[("a", D1), ("b", D2)])).await;
        let (st, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/feed")})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{out}");
        assert_eq!(out["feed"]["title"], "Show One");
        assert_eq!(out["episodes_added"], 2);
        assert_eq!(out["feed"]["unplayed_count"], 2);
        let id = out["feed"]["id"].as_i64().unwrap();
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{id}/episodes"),
            None,
        )
        .await;
        let titles: Vec<&str> = eps
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["Episode b", "Episode a"], "newest first");
        assert_eq!(eps[0]["duration_ms"], 600_000);
        assert_eq!(eps[0]["downloaded"], false);
        // The same feed again, even spelled differently, is one subscription.
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/feed#x")})),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        let (_, list) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn bad_addresses_are_refused_with_a_reason_and_nothing_is_saved() {
        let e = env().await;
        let (base, _s) = site(feed_xml("X", &[("a", D1)])).await;
        let (st, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/page")})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(
            out["error"].as_str().unwrap().contains("/feed"),
            "names the feed the page points to: {out}"
        );
        let (st, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/gone")})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        assert!(out["error"].as_str().unwrap().contains("gone"), "{out}");
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": "ftp://x/y"})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (_, list) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
        assert!(list.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refresh_adds_new_episodes_skips_unchanged_feeds_and_keeps_dropped_ones() {
        let e = env().await;
        let (base, site) = site(feed_xml("Show", &[("a", D1), ("b", D2)])).await;
        let (_, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/feed")})),
        )
        .await;
        let id = out["feed"]["id"].as_i64().unwrap();
        // Nothing changed: the server says 304 and no episodes are touched.
        let (_, r) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/refresh",
            Some(serde_json::json!({"feed_id": id})),
        )
        .await;
        assert_eq!(r[0]["unchanged"], true);
        // A new episode arrives and the oldest one is dropped from the feed.
        *site.feed.lock().unwrap() = feed_xml("Show renamed", &[("b", D2), ("c", D3)]);
        *site.etag.lock().unwrap() = Some("\"v2\"".into());
        let (_, r) = call(&e.app, Method::POST, "/api/podcasts/refresh", None).await;
        assert_eq!(r[0]["episodes_added"], 1);
        let (_, f) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
        assert_eq!(f[0]["title"], "Show renamed");
        assert_eq!(f[0]["episode_count"], 3, "the dropped episode is kept");
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{id}/episodes"),
            None,
        )
        .await;
        let a = eps
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["guid"] == "a")
            .unwrap();
        assert_eq!(a["dropped_from_feed"], true);
        // Played state: marking and filtering.
        let ep_id = eps[0]["id"].as_i64().unwrap();
        let (st, _) = call(
            &e.app,
            Method::POST,
            &format!("/api/podcasts/episodes/{ep_id}/played"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (_, unplayed) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{id}/episodes?unplayed=1"),
            None,
        )
        .await;
        assert_eq!(unplayed.as_array().unwrap().len(), 2);
        call(
            &e.app,
            Method::POST,
            &format!("/api/podcasts/episodes/{ep_id}/played"),
            Some(serde_json::json!({"played": false})),
        )
        .await;
        let (_, unplayed) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{id}/episodes?unplayed=1"),
            None,
        )
        .await;
        assert_eq!(unplayed.as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_failing_feed_keeps_its_episodes_and_shows_why() {
        let e = env().await;
        let (base, site) = site(feed_xml("Show", &[("a", D1)])).await;
        let (_, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/feed")})),
        )
        .await;
        let id = out["feed"]["id"].as_i64().unwrap();
        *site.etag.lock().unwrap() = None;
        *site.feed.lock().unwrap() = "<html><body>maintenance</body></html>".into();
        let (_, r) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/refresh",
            Some(serde_json::json!({"feed_id": id})),
        )
        .await;
        assert!(
            r[0]["error"]
                .as_str()
                .unwrap()
                .contains("not a podcast feed"),
            "{r}"
        );
        let (_, f) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
        assert_eq!(f[0]["episode_count"], 1, "nothing lost");
        assert!(
            f[0]["last_error"].as_str().is_some(),
            "the failure is on the feed"
        );
        // It recovers.
        *site.feed.lock().unwrap() = feed_xml("Show", &[("a", D1), ("b", D2)]);
        let (_, r) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/refresh",
            Some(serde_json::json!({"feed_id": id})),
        )
        .await;
        assert!(r[0]["error"].is_null());
        let (_, f) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
        assert!(f[0]["last_error"].is_null());
        assert_eq!(f[0]["episode_count"], 2);
    }

    #[tokio::test]
    async fn opml_import_adds_feeds_reads_them_in_the_background_and_export_round_trips() {
        let e = env().await;
        let (base, _s) = site(feed_xml("From OPML", &[("a", D1)])).await;
        let opml = format!(
            "<opml version=\"2.0\"><body><outline text=\"Named\" xmlUrl=\"{base}/feed\"/>\
             <outline text=\"Bad\" xmlUrl=\"ftp://nope/x\"/><outline text=\"Dead\" xmlUrl=\"{base}/gone\"/></body></opml>"
        );
        let (st, b) = call_raw(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds/import-opml",
            Some(("text/xml", opml.clone())),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let out: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(out["added"], 2);
        assert_eq!(out["invalid"], serde_json::json!(["ftp://nope/x"]));
        let (_, b) = call_raw(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds/import-opml",
            Some(("text/xml", opml)),
        )
        .await;
        let again: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(again["already_subscribed"], 2);
        // The background read fills them in: the good feed gets its episodes,
        // the dead one its error. Wait for both, they run side by side.
        let mut settled = false;
        for _ in 0..200 {
            let (_, f) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
            let list = f.as_array().unwrap();
            let good = list
                .iter()
                .any(|x| x["title"] == "From OPML" && x["episode_count"] == 1);
            let dead = list
                .iter()
                .any(|x| x["title"] == "Dead" && x["last_error"].as_str().is_some());
            if good && dead {
                settled = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(settled, "both imported feeds were read in the background");
        let (st, xml) =
            call_raw(&e.app, Method::GET, "/api/podcasts/feeds/export-opml", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            podcasts::parse_opml(&String::from_utf8(xml).unwrap()).len(),
            2
        );
        let (st, _) = call_raw(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds/import-opml",
            Some(("text/xml", "nothing".into())),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let _ = &e.state;
    }

    #[tokio::test]
    async fn unsubscribing_removes_the_feed_and_its_episodes() {
        let e = env().await;
        let (base, _s) = site(feed_xml("Show", &[("a", D1)])).await;
        let (_, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/feed")})),
        )
        .await;
        let id = out["feed"]["id"].as_i64().unwrap();
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/podcasts/feeds/{id}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let n: i64 = sqlx::query("SELECT COUNT(*) FROM podcast_episodes")
            .fetch_one(&e.state.pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(n, 0);
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/podcasts/feeds/{id}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{id}/episodes"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    fn audio_bytes() -> Vec<u8> {
        (0..50_000u32).map(|i| (i % 251) as u8).collect()
    }

    async fn subscribe(e: &Env, base: &str) -> i64 {
        let (st, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/feed")})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{out}");
        out["feed"]["id"].as_i64().unwrap()
    }

    fn use_folder(e: &Env) -> std::path::PathBuf {
        let dir = e._dir.path().join("podcasts");
        e.state.config.write().unwrap().podcast_dir = Some(dir.clone());
        dir
    }

    /// Wait until the episode's file exists (downloads run in the background).
    async fn wait_downloaded(e: &Env, feed: i64, n: usize) -> serde_json::Value {
        for _ in 0..200 {
            let (_, eps) = call(
                &e.app,
                Method::GET,
                &format!("/api/podcasts/feeds/{feed}/episodes"),
                None,
            )
            .await;
            if eps
                .as_array()
                .unwrap()
                .iter()
                .filter(|x| x["downloaded"] == true)
                .count()
                >= n
            {
                return eps;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the downloads did not finish");
    }

    async fn no_auto(e: &Env, feed: i64) {
        call(
            &e.app,
            Method::PUT,
            &format!("/api/podcasts/feeds/{feed}/settings"),
            Some(serde_json::json!({"auto_download": false})),
        )
        .await;
    }

    #[tokio::test]
    async fn new_episodes_download_by_themselves_into_show_folders() {
        let e = env().await;
        let dir = use_folder(&e);
        let (base, _s) = site(String::new()).await;
        *_s.feed.lock().unwrap() = feed_xml_at(&base, "My Show", &[("a", D1), ("b", D2)]);
        let feed = subscribe(&e, &base).await;
        let eps = wait_downloaded(&e, feed, 2).await;
        let path = dir.join("My Show").join("2026-03-03 - Episode b.mp3");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            audio_bytes(),
            "the whole file, intact"
        );
        assert!(dir
            .join("My Show")
            .join("2026-03-02 - Episode a.mp3")
            .is_file());
        assert!(eps[0]["downloaded"] == true);
        // The file is served with byte ranges.
        let id = eps[0]["id"].as_i64().unwrap();
        let (st, body) = call_raw(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/episodes/{id}/file"),
            None,
        )
        .await;
        assert_eq!((st, body.len()), (StatusCode::OK, 50_000));
        let req = Request::builder()
            .uri(format!("/api/podcasts/episodes/{id}/file"))
            .header("range", "bytes=100-199")
            .body(Body::empty())
            .unwrap();
        let res = e.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        let (st, _) = call_raw(
            &e.app,
            Method::GET,
            "/api/podcasts/episodes/9999/file",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        // Folder info.
        let (_, f) = call(&e.app, Method::GET, "/api/podcasts/folder", None).await;
        assert_eq!(f["episodes_downloaded"], 2);
        assert_eq!(f["bytes_downloaded"], 100_000);
        assert_eq!(f["usable"], true);
    }

    #[tokio::test]
    async fn an_interrupted_download_resumes_from_its_partial_file() {
        let e = env().await;
        let dir = use_folder(&e);
        let (base, s) = site(String::new()).await;
        *s.feed.lock().unwrap() = feed_xml_at(&base, "Resume", &[("a", D1)]);
        // Subscribe without downloading, then leave a partial file as a dropped download would.
        let (_, out) = call(
            &e.app,
            Method::POST,
            "/api/podcasts/feeds",
            Some(serde_json::json!({"url": format!("{base}/feed")})),
        )
        .await;
        let feed = out["feed"]["id"].as_i64().unwrap();
        no_auto(&e, feed).await;
        let eps = {
            // Auto-download may already have started; start over from a clean slate.
            let (_, eps) = call(
                &e.app,
                Method::GET,
                &format!("/api/podcasts/feeds/{feed}/episodes"),
                None,
            )
            .await;
            eps
        };
        let id = eps[0]["id"].as_i64().unwrap();
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/podcasts/episodes/{id}/download"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let target = dir.join("Resume").join("2026-03-02 - Episode a.mp3");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(format!("{}.part", target.display()), &audio_bytes()[..1000]).unwrap();
        let (st, job) = call(
            &e.app,
            Method::POST,
            &format!("/api/podcasts/episodes/{id}/download"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED, "{job}");
        wait_downloaded(&e, feed, 1).await;
        assert_eq!(
            std::fs::read(&target).unwrap(),
            audio_bytes(),
            "the first 1000 bytes were not fetched again"
        );
        assert!(!std::path::Path::new(&format!("{}.part", target.display())).exists());
        // Already there: asking again is a conflict, not a second copy.
        let (st, _) = call(
            &e.app,
            Method::POST,
            &format!("/api/podcasts/episodes/{id}/download"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_link_that_opens_a_web_page_fails_cleanly() {
        let e = env().await;
        let dir = use_folder(&e);
        let (base, s) = site(String::new()).await;
        *s.feed.lock().unwrap() = feed_xml_at(&base, "Paywall", &[("html1", D1)]);
        let feed = subscribe(&e, &base).await;
        no_auto(&e, feed).await;
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{feed}/episodes"),
            None,
        )
        .await;
        let id = eps[0]["id"].as_i64().unwrap();
        // Wait out any automatic attempt, then ask explicitly.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = call(
            &e.app,
            Method::POST,
            &format!("/api/podcasts/episodes/{id}/download"),
            None,
        )
        .await;
        let mut message = String::new();
        for _ in 0..100 {
            let (_, jobs) = call(&e.app, Method::GET, "/api/jobs", None).await;
            if let Some(j) = jobs.as_array().and_then(|a| {
                a.iter()
                    .find(|j| j["kind"] == "podcast_download" && j["status"] == "failed")
            }) {
                message = j["message"].as_str().unwrap_or("").to_string();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(message.contains("web page"), "{message}");
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{feed}/episodes"),
            None,
        )
        .await;
        assert_eq!(eps[0]["downloaded"], false);
        assert!(!dir
            .join("Paywall")
            .join("2026-03-02 - Episode html1.mp3")
            .exists());
    }

    #[tokio::test]
    async fn keep_n_clears_played_episodes_first_then_the_oldest() {
        let e = env().await;
        let dir = use_folder(&e);
        let (base, s) = site(String::new()).await;
        *s.feed.lock().unwrap() = feed_xml_at(&base, "Keep", &[("a", D1), ("b", D2), ("c", D3)]);
        let feed = subscribe(&e, &base).await;
        no_auto(&e, feed).await;
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{feed}/episodes"),
            None,
        )
        .await;
        let ids: std::collections::HashMap<String, i64> = eps
            .as_array()
            .unwrap()
            .iter()
            .map(|x| {
                (
                    x["guid"].as_str().unwrap().to_string(),
                    x["id"].as_i64().unwrap(),
                )
            })
            .collect();
        // All three on disk; the newest (c) was played.
        for (g, id) in &ids {
            let p = dir.join(format!("Keep/{g}.mp3"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
            sqlx::query("UPDATE podcast_episodes SET file_path = ? WHERE id = ?")
                .bind(p.to_string_lossy().to_string())
                .bind(id)
                .execute(&e.state.pool)
                .await
                .unwrap();
        }
        call(
            &e.app,
            Method::POST,
            &format!("/api/podcasts/episodes/{}/played", ids["c"]),
            None,
        )
        .await;
        // Keep 2, auto on: the played one goes, the older two stay.
        call(
            &e.app,
            Method::PUT,
            &format!("/api/podcasts/feeds/{feed}/settings"),
            Some(serde_json::json!({"auto_download": true, "keep_n": 2})),
        )
        .await;
        assert_eq!(
            podcast_dl::enforce_keep(&e.state.pool, feed).await.unwrap(),
            1
        );
        assert!(!dir.join("Keep/c.mp3").exists(), "played goes first");
        assert!(dir.join("Keep/a.mp3").exists() && dir.join("Keep/b.mp3").exists());
        // Keep 1: now the oldest unplayed goes.
        call(
            &e.app,
            Method::PUT,
            &format!("/api/podcasts/feeds/{feed}/settings"),
            Some(serde_json::json!({"keep_n": 1})),
        )
        .await;
        assert_eq!(
            podcast_dl::enforce_keep(&e.state.pool, feed).await.unwrap(),
            1
        );
        assert!(!dir.join("Keep/a.mp3").exists() && dir.join("Keep/b.mp3").exists());
        // The episodes themselves are still listed.
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{feed}/episodes"),
            None,
        )
        .await;
        assert_eq!(eps.as_array().unwrap().len(), 3);
        // With automatic downloading off nothing is cleared.
        call(
            &e.app,
            Method::PUT,
            &format!("/api/podcasts/feeds/{feed}/settings"),
            Some(serde_json::json!({"auto_download": false, "keep_n": 1})),
        )
        .await;
        assert_eq!(
            podcast_dl::enforce_keep(&e.state.pool, feed).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn played_files_are_cleared_after_the_chosen_number_of_days() {
        let e = env().await;
        let dir = use_folder(&e);
        let (base, s) = site(String::new()).await;
        *s.feed.lock().unwrap() =
            feed_xml_at(&base, "Tidy", &[("old", D1), ("new", D2), ("unplayed", D3)]);
        let feed = subscribe(&e, &base).await;
        no_auto(&e, feed).await;
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{feed}/episodes"),
            None,
        )
        .await;
        let now = crate::podcasts::now_ms();
        for x in eps.as_array().unwrap() {
            let g = x["guid"].as_str().unwrap();
            let p = dir.join(format!("Tidy/{g}.mp3"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
            let played = match g {
                "old" => Some(now - 10 * 86_400_000),
                "new" => Some(now - 3 * 86_400_000),
                _ => None,
            };
            sqlx::query("UPDATE podcast_episodes SET file_path = ?, played_at = ? WHERE id = ?")
                .bind(p.to_string_lossy().to_string())
                .bind(played)
                .bind(x["id"].as_i64().unwrap())
                .execute(&e.state.pool)
                .await
                .unwrap();
        }
        assert_eq!(
            podcast_dl::janitor(&e.state.pool, now).await.unwrap(),
            1,
            "default is 7 days"
        );
        assert!(!dir.join("Tidy/old.mp3").exists());
        assert!(dir.join("Tidy/new.mp3").exists() && dir.join("Tidy/unplayed.mp3").exists());
        // 0 means never.
        call(
            &e.app,
            Method::PUT,
            &format!("/api/podcasts/feeds/{feed}/settings"),
            Some(serde_json::json!({"delete_played_after_days": 0})),
        )
        .await;
        assert_eq!(
            podcast_dl::janitor(&e.state.pool, now + 400 * 86_400_000)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn deleting_a_download_removes_the_file_and_feed_settings_are_checked() {
        let e = env().await;
        let dir = use_folder(&e);
        let (base, s) = site(String::new()).await;
        *s.feed.lock().unwrap() = feed_xml_at(&base, "Del", &[("a", D1)]);
        let feed = subscribe(&e, &base).await;
        let eps = wait_downloaded(&e, feed, 1).await;
        let id = eps[0]["id"].as_i64().unwrap();
        let file = dir.join("Del").join("2026-03-02 - Episode a.mp3");
        assert!(file.is_file());
        no_auto(&e, feed).await;
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/podcasts/episodes/{id}/download"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        assert!(!file.exists());
        assert!(!dir.join("Del").exists(), "the empty show folder goes too");
        let (_, eps) = call(
            &e.app,
            Method::GET,
            &format!("/api/podcasts/feeds/{feed}/episodes"),
            None,
        )
        .await;
        assert_eq!(eps[0]["downloaded"], false);
        for bad in [
            serde_json::json!({"keep_n": 0}),
            serde_json::json!({"keep_n": 101}),
            serde_json::json!({"delete_played_after_days": 366}),
        ] {
            let (st, _) = call(
                &e.app,
                Method::PUT,
                &format!("/api/podcasts/feeds/{feed}/settings"),
                Some(bad),
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
        }
        let (st, _) = call(
            &e.app,
            Method::PUT,
            "/api/podcasts/feeds/999/settings",
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}
