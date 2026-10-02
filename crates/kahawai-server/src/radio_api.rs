//! `/api/radio/*`: the station directory (proxied and cached), the listener's
//! favorites, stream probing and the "heard" list (docs/v2/kahawai-radio-spec.md).
//!
//! The directory calls are the only online part and sit behind
//! `online_sources_enabled`. Favorites, manual stations, probing and history
//! work without it, so a saved station list never depends on a service.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use kahawai_core::MusicError;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePool, Row};

use crate::api::ApiError;
use crate::db::cvt;
use crate::radio::{self, Facet, Probe, SearchParams, Station};
use crate::AppState;

type ApiResult<T> = Result<T, ApiError>;

fn now_ms() -> i64 {
    crate::audiobooks::now_ms()
}

fn require_online(s: &AppState) -> Result<Option<String>, MusicError> {
    let cfg = s.config.read().unwrap();
    if !cfg.online_sources_enabled {
        return Err(MusicError::BadRequest(
            "the online station directory is off: turn on Online sources in the Server settings"
                .to_string(),
        ));
    }
    Ok(cfg.radio_browser_url.clone())
}

// --- directory ---------------------------------------------------------------

/// `GET /api/radio/search?q=&tag=&country=&language=&order=&limit=&offset=`
pub async fn search(
    State(s): State<AppState>,
    Query(p): Query<SearchParams>,
) -> ApiResult<Json<Vec<Station>>> {
    let forced = require_online(&s)?;
    let v = radio::directory_json(&s.pool, forced.as_deref(), &p.path(), true).await?;
    Ok(Json(radio::parse_stations(&v)))
}

/// `GET /api/radio/facets/{tags|countries|languages}`: what the pickers offer.
pub async fn facets(
    State(s): State<AppState>,
    Path(kind): Path<String>,
) -> ApiResult<Json<Vec<Facet>>> {
    let path = match kind.as_str() {
        "tags" => "/json/tags?hidebroken=true&order=stationcount&reverse=true&limit=300",
        "countries" => "/json/countries?hidebroken=true&order=stationcount&reverse=true",
        "languages" => "/json/languages?hidebroken=true&order=stationcount&reverse=true&limit=200",
        _ => return Err(MusicError::NotFound(format!("facet {kind}")).into()),
    };
    let forced = require_online(&s)?;
    let v = radio::directory_json(&s.pool, forced.as_deref(), path, true).await?;
    Ok(Json(radio::parse_facets(&v)))
}

// --- favorites ----------------------------------------------------------------

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Favorite {
    pub id: i64,
    pub station_uuid: Option<String>,
    pub name: String,
    pub url: String,
    pub url_resolved: Option<String>,
    pub homepage: Option<String>,
    pub favicon: Option<String>,
    pub tags: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
    pub bitrate: Option<i64>,
    pub codec: Option<String>,
    pub manual: bool,
    pub sort_order: i64,
    pub added_at: i64,
    /// AAC+ : the server has to decode it.
    pub needs_relay: bool,
}

const FAV_COLS: &str =
    "id, station_uuid, name, url, url_resolved, homepage, favicon, tags, country,
     language, bitrate, codec, manual, sort_order, added_at";

fn favorite_from_row(r: &sqlx::sqlite::SqliteRow) -> Favorite {
    let codec: Option<String> = r.get("codec");
    Favorite {
        id: r.get("id"),
        station_uuid: r.get("station_uuid"),
        name: r.get("name"),
        url: r.get("url"),
        url_resolved: r.get("url_resolved"),
        homepage: r.get("homepage"),
        favicon: r.get("favicon"),
        tags: r.get("tags"),
        country: r.get("country"),
        language: r.get("language"),
        bitrate: r.get("bitrate"),
        needs_relay: codec.as_deref().is_some_and(radio::is_he_aac),
        codec,
        manual: r.get::<i64, _>("manual") != 0,
        sort_order: r.get("sort_order"),
        added_at: r.get("added_at"),
    }
}

async fn favorite(pool: &SqlitePool, id: i64) -> Result<Favorite, MusicError> {
    sqlx::query(&format!(
        "SELECT {FAV_COLS} FROM radio_favorites WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(cvt)?
    .map(|r| favorite_from_row(&r))
    .ok_or_else(|| MusicError::NotFound(format!("station {id}")))
}

pub async fn list_favorites(State(s): State<AppState>) -> ApiResult<Json<Vec<Favorite>>> {
    Ok(Json(
        sqlx::query(&format!(
            "SELECT {FAV_COLS} FROM radio_favorites ORDER BY sort_order, id"
        ))
        .fetch_all(&s.pool)
        .await
        .map_err(cvt)?
        .iter()
        .map(favorite_from_row)
        .collect(),
    ))
}

#[derive(Deserialize)]
pub struct NewFavorite {
    /// Present for a directory station, absent for one typed in.
    #[serde(default)]
    pub station_uuid: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub url: String,
    #[serde(default)]
    pub url_resolved: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub favicon: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub bitrate: Option<i64>,
    #[serde(default)]
    pub codec: Option<String>,
}

fn blank(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `POST /api/radio/favorites`: save a directory station, or add one by its
/// address. A typed-in address is probed first, which fills the codec and
/// bitrate and name when the stream says them, and refuses a dead address,
/// a playlist or a non-stream rather than saving something that cannot play.
pub async fn add_favorite(
    State(s): State<AppState>,
    Json(b): Json<NewFavorite>,
) -> ApiResult<Response> {
    let url = b.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(MusicError::BadRequest(
            "a stream address starts with http:// or https://".into(),
        )
        .into());
    }
    let uuid = blank(b.station_uuid);
    let manual = uuid.is_none();
    let (mut name, mut codec, mut bitrate) = (blank(b.name), blank(b.codec), b.bitrate);
    let mut tags = blank(b.tags);
    if manual {
        let p = radio::probe(&url).await?;
        if p.playlist {
            return Err(MusicError::BadRequest(
                "that address is a playlist (.pls / .m3u); paste the stream address inside it"
                    .into(),
            )
            .into());
        }
        name = name.or(p.name);
        codec = codec.or(p.codec);
        bitrate = bitrate.or(p.bitrate);
        tags = tags.or(p.genre);
    }
    let name = name.unwrap_or_else(|| url.clone());
    let next: i64 = sqlx::query("SELECT COALESCE(MAX(sort_order), 0) + 1 FROM radio_favorites")
        .fetch_one(&s.pool)
        .await
        .map_err(cvt)?
        .get(0);
    let row = sqlx::query(&format!(
        "INSERT INTO radio_favorites (station_uuid, name, url, url_resolved, homepage, favicon, tags,
           country, language, bitrate, codec, manual, sort_order, added_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING {FAV_COLS}"
    ))
    .bind(uuid)
    .bind(name)
    .bind(url)
    .bind(blank(b.url_resolved))
    .bind(blank(b.homepage))
    .bind(blank(b.favicon))
    .bind(tags)
    .bind(blank(b.country))
    .bind(blank(b.language))
    .bind(bitrate.filter(|b| *b > 0))
    .bind(codec)
    .bind(i64::from(manual))
    .bind(next)
    .bind(now_ms())
    .fetch_one(&s.pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            MusicError::Conflict("that station is already a favorite".into())
        }
        e => cvt(e),
    })?;
    Ok((StatusCode::CREATED, Json(favorite_from_row(&row))).into_response())
}

#[derive(Deserialize)]
pub struct FavoriteEdit {
    pub name: Option<String>,
}

pub async fn edit_favorite(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(e): Json<FavoriteEdit>,
) -> ApiResult<Json<Favorite>> {
    favorite(&s.pool, id).await?;
    if let Some(name) = blank(e.name) {
        sqlx::query("UPDATE radio_favorites SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&s.pool)
            .await
            .map_err(cvt)?;
    }
    Ok(Json(favorite(&s.pool, id).await?))
}

pub async fn delete_favorite(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let n = sqlx::query("DELETE FROM radio_favorites WHERE id = ?")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(cvt)?
        .rows_affected();
    if n == 0 {
        return Err(MusicError::NotFound(format!("station {id}")).into());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct Order {
    pub ids: Vec<i64>,
}

/// `PUT /api/radio/favorites/order`: the list, in the order wanted. Ids not
/// named keep their relative order after the named ones.
pub async fn reorder_favorites(
    State(s): State<AppState>,
    Json(o): Json<Order>,
) -> ApiResult<Json<Vec<Favorite>>> {
    let mut tx = s.pool.begin().await.map_err(cvt)?;
    let rest: Vec<i64> = sqlx::query("SELECT id FROM radio_favorites ORDER BY sort_order, id")
        .fetch_all(&mut *tx)
        .await
        .map_err(cvt)?
        .iter()
        .map(|r| r.get(0))
        .filter(|id| !o.ids.contains(id))
        .collect();
    for (i, id) in o.ids.iter().chain(rest.iter()).enumerate() {
        sqlx::query("UPDATE radio_favorites SET sort_order = ? WHERE id = ?")
            .bind(i as i64 + 1)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(cvt)?;
    }
    tx.commit().await.map_err(cvt)?;
    list_favorites(State(s)).await
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PlayInfo {
    pub name: String,
    /// What to connect to.
    pub url: String,
    pub codec: Option<String>,
    pub bitrate: Option<i64>,
    pub needs_relay: bool,
}

/// `POST /api/radio/favorites/{id}/play`: the address to connect to. With the
/// online directory on, a station from it is asked for its current address
/// (which also counts the listen for the directory); any trouble there, or the
/// directory being off, plays the address that was saved.
pub async fn play_favorite(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<PlayInfo>> {
    let f = favorite(&s.pool, id).await?;
    let saved = f.url_resolved.clone().unwrap_or_else(|| f.url.clone());
    let mut url = saved;
    let online = {
        let cfg = s.config.read().unwrap();
        cfg.online_sources_enabled
            .then(|| cfg.radio_browser_url.clone())
    };
    if let (Some(forced), Some(uuid)) = (online, f.station_uuid.as_deref()) {
        match radio::resolve_station_url(forced.as_deref(), uuid).await {
            Ok(Some(fresh)) => url = fresh,
            Ok(None) => {}
            Err(e) => {
                tracing::info!(error = %e, "station address lookup failed; using the saved one")
            }
        }
    }
    Ok(Json(PlayInfo {
        name: f.name,
        url,
        needs_relay: f.needs_relay,
        codec: f.codec,
        bitrate: f.bitrate,
    }))
}

// --- probe --------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ProbeBody {
    pub url: String,
}

/// `POST /api/radio/probe {url}`: what a stream says about itself.
pub async fn probe(Json(b): Json<ProbeBody>) -> ApiResult<Json<Probe>> {
    Ok(Json(radio::probe(&b.url).await?))
}

// --- history ------------------------------------------------------------------

#[derive(Deserialize)]
pub struct HeardBody {
    pub station_name: String,
    pub stream_title: String,
}

/// `POST /api/radio/history`: the Player reports a new track title. The same
/// title straight after itself is not logged twice.
pub async fn add_history(
    State(s): State<AppState>,
    Json(b): Json<HeardBody>,
) -> ApiResult<StatusCode> {
    let (station, title) = (b.station_name.trim(), b.stream_title.trim());
    if station.is_empty() || title.is_empty() {
        return Err(MusicError::BadRequest("a station and a title are needed".into()).into());
    }
    let last: Option<String> = sqlx::query(
        "SELECT stream_title FROM radio_history WHERE station_name = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(station)
    .fetch_optional(&s.pool)
    .await
    .map_err(cvt)?
    .map(|r| r.get(0));
    if last.as_deref() == Some(title) {
        return Ok(StatusCode::NO_CONTENT);
    }
    sqlx::query(
        "INSERT INTO radio_history (station_name, stream_title, played_at) VALUES (?, ?, ?)",
    )
    .bind(station)
    .bind(title)
    .bind(now_ms())
    .execute(&s.pool)
    .await
    .map_err(cvt)?;
    Ok(StatusCode::CREATED)
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Heard {
    pub id: i64,
    pub station_name: String,
    pub stream_title: String,
    pub played_at: i64,
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
}

pub async fn history(
    State(s): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<Heard>>> {
    Ok(Json(
        sqlx::query(
            "SELECT id, station_name, stream_title, played_at FROM radio_history
             ORDER BY id DESC LIMIT ?",
        )
        .bind(q.limit.unwrap_or(100).clamp(1, 1000))
        .fetch_all(&s.pool)
        .await
        .map_err(cvt)?
        .iter()
        .map(|r| Heard {
            id: r.get("id"),
            station_name: r.get("station_name"),
            stream_title: r.get("stream_title"),
            played_at: r.get("played_at"),
        })
        .collect(),
    ))
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
    use std::sync::Arc;
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
        let mut req = Request::builder().method(method).uri(uri);
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
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// A stand-in radio-browser.info; counts what it was asked.
    async fn directory() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let app = Router::new().fallback(get(move |uri: axum::http::Uri| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(uri.to_string());
                let p = uri.path();
                let body = if p.starts_with("/json/stations/search") {
                    serde_json::json!([
                        {"stationuuid": "aaaa-1", "name": "Jazz One", "url": "http://j/pls",
                         "url_resolved": "http://j/live", "codec": "MP3", "bitrate": 128,
                         "tags": "jazz", "clickcount": 5, "hls": 0},
                        {"stationuuid": "bbbb-2", "name": "Plus", "url": "http://p/live",
                         "codec": "AAC+", "bitrate": 64}
                    ])
                } else if p.starts_with("/json/url/") {
                    serde_json::json!({"ok": true, "url": "http://fresh/live"})
                } else if p.starts_with("/json/tags") {
                    serde_json::json!([{"name": "jazz", "stationcount": 10}])
                } else {
                    serde_json::json!([])
                };
                axum::Json(body)
            }
        }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (base, seen)
    }

    /// A stream that greets like Shoutcast v1 and then plays forever.
    async fn stream() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/live", l.local_addr().unwrap());
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut b = [0u8; 1024];
                    let _ = s.read(&mut b).await;
                    let _ = s
                        .write_all(
                            b"ICY 200 OK\r\nicy-name: Test FM\r\nicy-br: 96\r\nicy-metaint: 8192\r\n\
                              content-type: audio/mpeg\r\n\r\n",
                        )
                        .await;
                    loop {
                        if s.write_all(&[0u8; 512]).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn the_directory_is_off_until_it_is_turned_on() {
        let e = env().await;
        let (st, body) = call(&e.app, Method::GET, "/api/radio/search?q=jazz", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(body.is_null() || body.to_string().contains("off"), "{body}");
        let (st, _) = call(&e.app, Method::GET, "/api/radio/facets/tags", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // Favorites, history and probing are not behind it.
        let (st, _) = call(&e.app, Method::GET, "/api/radio/favorites", None).await;
        assert_eq!(st, StatusCode::OK);
    }

    #[tokio::test]
    async fn search_is_proxied_and_cached_for_a_day() {
        let e = env().await;
        let (base, seen) = directory().await;
        {
            let mut c = e.state.config.write().unwrap();
            c.online_sources_enabled = true;
            c.radio_browser_url = Some(base);
        }
        let (st, v) = call(
            &e.app,
            Method::GET,
            "/api/radio/search?q=Jazz&limit=10",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let v = v.as_array().unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["name"], "Jazz One");
        assert_eq!(v[1]["needs_relay"], true);
        // Spelled differently, it is the same cached question.
        call(
            &e.app,
            Method::GET,
            "/api/radio/search?q=jazz&limit=10",
            None,
        )
        .await;
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "one request to the directory"
        );
        let (_, t) = call(&e.app, Method::GET, "/api/radio/facets/tags", None).await;
        assert_eq!(t[0]["name"], "jazz");
        let (st, _) = call(&e.app, Method::GET, "/api/radio/facets/nope", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_down_directory_serves_the_old_answer_or_a_clear_error() {
        let e = env().await;
        {
            let mut c = e.state.config.write().unwrap();
            c.online_sources_enabled = true;
            c.radio_browser_url = Some("http://127.0.0.1:1".into());
        }
        let (st, _) = call(&e.app, Method::GET, "/api/radio/search?q=x", None).await;
        assert_eq!(
            st,
            StatusCode::BAD_GATEWAY,
            "nothing cached, nothing reachable"
        );
        // An old (expired) answer stands in.
        let path = SearchParams {
            q: Some("x".into()),
            ..Default::default()
        }
        .path();
        sqlx::query("INSERT INTO radio_cache (key, body, fetched_at) VALUES (?, ?, 1)")
            .bind(&path)
            .bind(r#"[{"stationuuid":"z","name":"Old","url":"http://o/"}]"#)
            .execute(&e.state.pool)
            .await
            .unwrap();
        let (st, v) = call(&e.app, Method::GET, "/api/radio/search?q=x", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v[0]["name"], "Old");
    }

    #[tokio::test]
    async fn favorites_are_saved_renamed_ordered_and_removed() {
        let e = env().await;
        let mk = |n: &str, u: &str| serde_json::json!({"station_uuid": n, "name": n, "url": u});
        let mut ids = vec![];
        for n in ["a", "b", "c"] {
            let (st, f) = call(
                &e.app,
                Method::POST,
                "/api/radio/favorites",
                Some(mk(n, &format!("http://{n}/"))),
            )
            .await;
            assert_eq!(st, StatusCode::CREATED);
            assert_eq!(f["manual"], false);
            ids.push(f["id"].as_i64().unwrap());
        }
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/radio/favorites",
            Some(mk("a", "http://a/")),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "the same station twice");
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/radio/favorites",
            Some(serde_json::json!({"url": "ftp://nope"})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        call(
            &e.app,
            Method::PUT,
            "/api/radio/favorites/order",
            Some(serde_json::json!({"ids": [ids[2], ids[0]]})),
        )
        .await;
        let (_, list) = call(&e.app, Method::GET, "/api/radio/favorites", None).await;
        let order: Vec<i64> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_i64().unwrap())
            .collect();
        assert_eq!(
            order,
            [ids[2], ids[0], ids[1]],
            "named first, the rest after"
        );

        let (_, f) = call(
            &e.app,
            Method::PATCH,
            &format!("/api/radio/favorites/{}", ids[1]),
            Some(serde_json::json!({"name": " Better "})),
        )
        .await;
        assert_eq!(f["name"], "Better");
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/radio/favorites/{}", ids[1]),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call(
            &e.app,
            Method::DELETE,
            &format!("/api/radio/favorites/{}", ids[1]),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_typed_in_station_is_probed_and_filled_in() {
        let e = env().await;
        let url = stream().await;
        let (st, p) = call(
            &e.app,
            Method::POST,
            "/api/radio/probe",
            Some(serde_json::json!({"url": url})),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(p["name"], "Test FM");
        assert_eq!(p["metaint"], 8192);
        let (st, f) = call(
            &e.app,
            Method::POST,
            "/api/radio/favorites",
            Some(serde_json::json!({"url": url})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(f["name"], "Test FM");
        assert_eq!(f["codec"], "MP3");
        assert_eq!(f["bitrate"], 96);
        assert_eq!(f["manual"], true);
        assert!(f["station_uuid"].is_null());
        // A dead address is refused, not saved.
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/radio/favorites",
            Some(serde_json::json!({"url": "http://127.0.0.1:1/x"})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_GATEWAY);
        let (_, list) = call(&e.app, Method::GET, "/api/radio/favorites", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn play_uses_the_directorys_fresh_address_or_the_saved_one() {
        let e = env().await;
        let (_, f) = call(
            &e.app,
            Method::POST,
            "/api/radio/favorites",
            Some(
                serde_json::json!({"station_uuid": "aaaa-1", "name": "J", "url": "http://old/pls",
                                     "url_resolved": "http://old/live", "codec": "AAC+"}),
            ),
        )
        .await;
        let id = f["id"].as_i64().unwrap();
        // Directory off: the saved (resolved) address.
        let (_, p) = call(
            &e.app,
            Method::POST,
            &format!("/api/radio/favorites/{id}/play"),
            None,
        )
        .await;
        assert_eq!(p["url"], "http://old/live");
        assert_eq!(p["needs_relay"], true);
        // Directory on: the fresh one.
        let (base, seen) = directory().await;
        {
            let mut c = e.state.config.write().unwrap();
            c.online_sources_enabled = true;
            c.radio_browser_url = Some(base);
        }
        let (_, p) = call(
            &e.app,
            Method::POST,
            &format!("/api/radio/favorites/{id}/play"),
            None,
        )
        .await;
        assert_eq!(p["url"], "http://fresh/live");
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|u| u.starts_with("/json/url/aaaa-1")));
        // Directory unreachable: back to the saved one.
        e.state.config.write().unwrap().radio_browser_url = Some("http://127.0.0.1:1".into());
        let (_, p) = call(
            &e.app,
            Method::POST,
            &format!("/api/radio/favorites/{id}/play"),
            None,
        )
        .await;
        assert_eq!(p["url"], "http://old/live");
    }

    #[tokio::test]
    async fn heard_titles_are_logged_without_immediate_repeats() {
        let e = env().await;
        let body = |t: &str| serde_json::json!({"station_name": "FM", "stream_title": t});
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/radio/history",
            Some(body("A - One")),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let (st, _) = call(
            &e.app,
            Method::POST,
            "/api/radio/history",
            Some(body("A - One")),
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT, "same title again");
        call(
            &e.app,
            Method::POST,
            "/api/radio/history",
            Some(body("B - Two")),
        )
        .await;
        call(
            &e.app,
            Method::POST,
            "/api/radio/history",
            Some(body("A - One")),
        )
        .await;
        let (_, h) = call(&e.app, Method::GET, "/api/radio/history?limit=10", None).await;
        let titles: Vec<&str> = h
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["stream_title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["A - One", "B - Two", "A - One"], "newest first");
        let (st, _) = call(&e.app, Method::POST, "/api/radio/history", Some(body("  "))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
