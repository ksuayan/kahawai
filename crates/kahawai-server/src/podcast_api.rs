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
    Ok(Json(refresh_many(&s.pool, ids).await))
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
        let items: String = eps
            .iter()
            .map(|(g, d)| {
                format!(
                    "<item><title>Episode {g}</title><guid>{g}</guid><pubDate>{d}</pubDate>\
                     <enclosure url=\"https://cdn.example/{g}.mp3\" length=\"10\" type=\"audio/mpeg\"/>\
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
            .route("/gone", get(|| async { StatusCode::NOT_FOUND }));
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
        // The background read fills them in.
        let mut named_ok = false;
        for _ in 0..100 {
            let (_, f) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
            named_ok = f
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x["title"] == "From OPML" && x["episode_count"] == 1);
            if named_ok {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(named_ok, "the imported feed was read");
        let (_, f) = call(&e.app, Method::GET, "/api/podcasts/feeds", None).await;
        let dead = f
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["title"] == "Dead")
            .unwrap();
        assert!(
            dead["last_error"].as_str().is_some(),
            "a dead feed shows its error: {dead}"
        );
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
}
