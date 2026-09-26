//! kahawai-server: self-hosted music streaming server binary.
//! (Spec: kahawai-spec.md)

mod api;
mod db;
mod dop;
mod dsd;
mod dsd_meta;
mod jobs;
mod resample;
mod scanner;
mod stream;
mod transcode;

use std::sync::Arc;

use axum::{
    error_handling::HandleErrorLayer,
    http::StatusCode,
    routing::{delete, get, post, put},
    BoxError, Router,
};
use kahawai_core::config::ServerConfig;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub jobs: jobs::JobStore,
    pub config: ServerConfig,
    /// Ensures only one library scan runs at a time (S1).
    pub scan_lock: Arc<tokio::sync::Mutex<()>>,
}

/// Request bodies larger than this are rejected with 413 (S10).
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;
/// API request timeout (S10): 60s covers even large playlist imports.
/// Long-lived `/stream/*` responses are deliberately excluded — a transcode
/// can run for the length of an album.
const API_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub fn app(state: AppState) -> Router {
    // S10: the JSON API gets a body cap and a request timeout. Streams are
    // routed separately so neither applies to long-lived audio responses.
    let api = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/albums", get(api::list_albums))
        .route("/api/albums/{id}", get(api::get_album))
        .route("/api/artists", get(api::list_artists))
        .route("/api/artists/{id}", get(api::get_artist))
        .route("/api/tracks/{id}", get(api::get_track))
        .route("/api/search", get(api::search))
        .route(
            "/api/playlists",
            get(api::list_playlists).post(api::create_playlist),
        )
        .route("/api/playlists/import", post(api::import_playlist))
        .route(
            "/api/playlists/{id}",
            delete(api::delete_playlist).patch(api::rename_playlist),
        )
        .route("/api/playlists/{id}/tracks", put(api::set_playlist_tracks))
        .route("/api/artwork/{hash}", get(api::artwork))
        .route("/api/scan", post(api::trigger_scan))
        .route("/api/jobs", get(api::list_jobs).post(api::create_job))
        .route("/api/jobs/{id}", get(api::get_job))
        // Timeout errors become 408 via the HandleErrorLayer (tower's
        // timeout error cannot convert to Infallible for Router::layer).
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|err: BoxError| async move {
                    warn!(error = %err, "API request timed out");
                    StatusCode::REQUEST_TIMEOUT
                }))
                .layer(tower::timeout::TimeoutLayer::new(API_TIMEOUT)),
        )
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES));

    Router::new()
        .merge(api)
        .route(
            "/stream/{id}",
            get(api::stream_track).head(api::stream_head),
        )
        .with_state(state)
        // v1 is LAN-only: permissive CORS is acceptable here (spec §3.6).
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".into());
    let config = match ServerConfig::load(&config_path) {
        Ok(c) => {
            info!(path = %config_path, "loaded config");
            c
        }
        Err(e) => {
            warn!(error = %e, path = %config_path, "using default config");
            ServerConfig::default()
        }
    };

    let pool = db::open(&config.db_path).await?;
    info!("SQLite catalog open (WAL mode)");

    // S9: durable jobs. Applies the restart rule (in-flight → failed with
    // "server restarted") before serving anything.
    let jobs = jobs::JobStore::persistent(&pool)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    let state = AppState {
        pool,
        jobs,
        config: config.clone(),
        scan_lock: Arc::new(tokio::sync::Mutex::new(())),
    };

    // S9: the startup scan is a real persisted job (visible in /api/jobs),
    // not a bare background task — it survives the same lifecycle, progress,
    // and restart rules as API-triggered scans.
    if config.scan_on_startup && !config.music_dirs.is_empty() {
        match state.scan_lock.clone().try_lock_owned() {
            Ok(guard) => {
                let job = state
                    .jobs
                    .create(
                        kahawai_core::JobKind::Scan,
                        "startup scan".to_string(),
                        None,
                    )
                    .await;
                api::spawn_scan_job(state.clone(), job.clone(), guard);
                info!(job_id = %job.id, "startup scan queued as persistent job");
            }
            Err(_) => warn!("startup scan skipped: a scan is already running"),
        }
    } else {
        info!("startup scan disabled (set scan_on_startup = true in config to enable)");
    }

    let addr: std::net::SocketAddr = config.bind.parse()?;
    // S10: the LAN-only posture is logged at every startup, not just in docs.
    warn!(
        %addr,
        "LAN-only build: no authentication, no TLS — bind to a trusted network only"
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("shutdown complete");
    Ok(())
}

/// SIGINT/SIGTERM → graceful shutdown: in-flight streams drain, then the
/// process exits. Jobs left running/queued in the DB are failed by the S9
/// restart rule on the next boot — never silently resumed.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("shutdown signal received; draining in-flight requests");
}

#[cfg(test)]
mod integration_tests {
    //! End-to-end through the real router with a temp SQLite DB and a
    //! fixture audio file. Hermetic: temp dirs only, no network.
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use sqlx::Row;
    use tower::ServiceExt;

    /// Build a test app: temp dir, fixture file of `fixture_len` patterned
    /// bytes, SQLite DB with one track row pointing at it.
    async fn test_app(fixture_len: usize) -> (Router, AppState, tempfile::TempDir, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let fixture: Vec<u8> = (0..fixture_len).map(|i| (i % 251) as u8).collect();
        let audio_path = dir.path().join("track01.mp3");
        std::fs::write(&audio_path, &fixture).unwrap();

        let db_path = dir.path().join("test.db");
        let pool = db::open(&db_path).await.unwrap();
        let id = db::insert_track_minimal(&pool, audio_path.to_str().unwrap(), "abc123", "mp3")
            .await
            .unwrap();
        assert_eq!(id, 1);

        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            // S10: tests run with the temp dir as the only music root, so
            // strict root enforcement is exercised, not bypassed.
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        (app(state.clone()), state, dir, fixture)
    }

    /// Test app backed by a real scan of the ffmpeg fixture library
    /// (3 albums incl. one compilation, 8 tracks). See scanner::fixtures.
    async fn scanned_app() -> (Router, AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let lib = crate::scanner::fixtures::build_library(dir.path());
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        let report = crate::scanner::run_scan_with_progress(&pool, &[lib], |_, _| {})
            .await
            .unwrap();
        assert_eq!(report.files_added, 8);
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        (app(state.clone()), state, dir)
    }

    async fn body_bytes(res: axum::response::Response) -> Vec<u8> {
        to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (app, _state, _dir, _fixture) = test_app(16).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_bytes(res).await;
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn stream_full_file_200() {
        let (app, _state, _dir, fixture) = test_app(2048).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/mpeg");
        let body = body_bytes(res).await;
        assert_eq!(body, fixture);
    }

    #[tokio::test]
    async fn stream_range_request_206() {
        let (app, _state, _dir, fixture) = test_app(2048).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .header(header::RANGE, "bytes=100-199")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 100-199/2048");
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "100");
        let body = body_bytes(res).await;
        assert_eq!(body.len(), 100);
        assert_eq!(body, &fixture[100..200]);
    }

    #[tokio::test]
    async fn stream_suffix_range_206() {
        let (app, _state, _dir, fixture) = test_app(2048).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .header(header::RANGE, "bytes=-50")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 1998-2047/2048");
        let body = body_bytes(res).await;
        assert_eq!(body, &fixture[1998..2048]);
    }

    #[tokio::test]
    async fn stream_unsatisfiable_range_416() {
        let (app, _state, _dir, _fixture) = test_app(2048).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .header(header::RANGE, "bytes=99999-")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes */2048");
    }

    #[tokio::test]
    async fn stream_missing_track_404() {
        let (app, _state, _dir, _fixture) = test_app(16).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    // Opus encoding is feature-gated: only meaningful on the default
    // build (with `encode-opus` the request proceeds to the encoder).
    #[cfg(not(feature = "encode-opus"))]
    #[tokio::test]
    async fn stream_transcode_format_501() {
        let (app, _state, _dir, _fixture) = test_app(16).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=opus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Opus encoding is feature-gated: the default build answers 501
        // and names the cargo feature that would enable it.
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
        let body = body_bytes(res).await;
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["feature"], "encode-opus");
    }

    #[cfg(not(feature = "encode-mp3"))]
    #[tokio::test]
    async fn stream_mp3_format_501_names_feature() {
        let (app, _state, _dir, _fixture) = test_app(16).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=mp3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
        let body = body_bytes(res).await;
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["feature"], "encode-mp3");
    }

    #[tokio::test]
    async fn stream_head_returns_headers_only() {
        let (app, _state, _dir, _fixture) = test_app(2048).await;
        let res = app
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "2048");
        assert_eq!(res.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(chain_of(&res), "mp3->passthrough");
        let body = body_bytes(res).await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn stream_head_transcode_reports_target_mime_and_chain() {
        let (app, _dir) = audio_app(
            ServerConfig::default(),
            dsf_fixture(8, 4096, 0xFE),
            "tone.dsf",
            "dsf",
        )
        .await;
        let res = app
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        // Mirrors GET: FLAC target MIME, no Accept-Ranges (a live transcode
        // cannot be ranged). hyper synthesizes `content-length: 0` for the
        // empty HEAD body itself — the GET body stays chunked/unknown-length.
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "0");
        assert!(!res.headers().contains_key(header::ACCEPT_RANGES));
        let chain = chain_of(&res);
        assert!(chain.starts_with("dsf64->"), "chain: {chain}");
        assert!(chain.contains("flac"), "chain: {chain}");
        let body = body_bytes(res).await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn playlists_crud_round_trip() {
        let (app, _state, _dir, _fixture) = test_app(16).await;

        // Create with the one track.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"queue save","track_ids":[1]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_bytes(res).await;
        let pl: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(pl["name"], "queue save");
        assert_eq!(pl["track_ids"], serde_json::json!([1]));
        let id = pl["id"].as_i64().unwrap();

        // List contains it.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/playlists")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_bytes(res).await;
        let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);

        // Delete.
        let res = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/playlists/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn playlist_rename_round_trip() {
        let (app, _state, _dir, _fixture) = test_app(16).await;

        // Create.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"old name","track_ids":[1]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_bytes(res).await;
        let pl: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = pl["id"].as_i64().unwrap();

        // Rename.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/playlists/{id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"new name"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_bytes(res).await;
        let pl: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(pl["name"], "new name");
        assert_eq!(pl["track_ids"], serde_json::json!([1]), "tracks untouched");

        // Empty name is rejected.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/playlists/{id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"  "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // Unknown id is 404.
        let res = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/playlists/424242")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn jobs_create_and_poll() {
        let (app, _state, _dir, _fixture) = test_app(16).await;

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/jobs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"kind":"transcode","label":"test transcode"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_bytes(res).await;
        let job: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(job["kind"], "transcode");
        assert_eq!(job["status"], "queued");
        let id = job["id"].as_str().unwrap().to_string();

        // Poll: the background ticker should drive it to done quickly enough
        // (20 steps x 250ms = 5s max; poll up to ~7s).
        let mut done = false;
        for _ in 0..70 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/jobs/{id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = body_bytes(res).await;
            let cur: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if cur["status"] == "done" {
                done = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(done, "background ticker did not finish the job");
    }

    // ------------------------------------------------------------------
    // S2 browse API over a real scanned catalog
    // ------------------------------------------------------------------

    async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = body_bytes(res).await;
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn albums_pagination() {
        let (app, _state, _dir) = scanned_app().await;

        let (status, v) = get_json(&app, "/api/albums?per_page=2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["page"], 1);
        assert_eq!(v["per_page"], 2);
        assert_eq!(v["total"], 3);
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
        // Sorted by title; each carries a track count, not full id lists.
        assert_eq!(v["items"][0]["title"], "Blue Train");
        assert_eq!(v["items"][0]["track_count"], 3);
        assert_eq!(v["items"][1]["title"], "Jazz Compilation");
        assert!(v["items"][0]["track_ids"].as_array().unwrap().is_empty());

        let (status, v) = get_json(&app, "/api/albums?page=2&per_page=2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["items"][0]["title"], "Kind of Blue");
    }

    #[tokio::test]
    async fn album_detail_has_ordered_tracks() {
        let (app, _state, _dir) = scanned_app().await;
        let (_, v) = get_json(&app, "/api/albums").await;
        let id = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["title"] == "Blue Train")
            .unwrap()["id"]
            .as_i64()
            .unwrap();

        let (status, d) = get_json(&app, &format!("/api/albums/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(d["album"]["title"], "Blue Train");
        assert_eq!(d["album"]["artist"], "John Coltrane");
        assert_eq!(d["album"]["year"], 1957);
        assert!(d["album"]["artwork_hash"].as_str().unwrap().len() == 64);
        let tracks = d["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 3);
        // Play order: disc/track number.
        let titles: Vec<&str> = tracks
            .iter()
            .map(|t| t["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, vec!["Blue Train", "Moment's Notice", "Locomotion"]);
        assert_eq!(d["album"]["track_ids"].as_array().unwrap().len(), 3);

        let (status, _) = get_json(&app, "/api/albums/9999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn artists_and_artist_detail() {
        let (app, _state, _dir) = scanned_app().await;
        let (status, v) = get_json(&app, "/api/artists").await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<&str> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        for expected in [
            "John Coltrane",
            "Miles Davis",
            "Dave Brubeck",
            "Thelonious Monk",
            "Dizzy Gillespie",
            "Various Artists",
        ] {
            assert!(names.contains(&expected), "missing artist {expected}");
        }

        // Miles Davis appears on Kind of Blue and on the compilation
        // (via the ";" split on "My Favorite Things").
        let id = v
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == "Miles Davis")
            .unwrap()["id"]
            .as_i64()
            .unwrap();
        let (status, d) = get_json(&app, &format!("/api/artists/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        let album_titles: Vec<&str> = d["albums"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["title"].as_str().unwrap())
            .collect();
        assert!(album_titles.contains(&"Kind of Blue"));
        assert!(album_titles.contains(&"Jazz Compilation"));
    }

    #[tokio::test]
    async fn track_detail() {
        let (app, _state, _dir) = scanned_app().await;
        let (_, v) = get_json(&app, "/api/albums").await;
        let album_id = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["title"] == "Kind of Blue")
            .unwrap()["id"]
            .as_i64()
            .unwrap();
        let (_, d) = get_json(&app, &format!("/api/albums/{album_id}")).await;
        let track_id = d["tracks"][0]["id"].as_i64().unwrap();

        let (status, t) = get_json(&app, &format!("/api/tracks/{track_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(t["title"], "So What");
        assert_eq!(t["format"], "mp3");
        assert_eq!(t["genre"], "Jazz");
        assert_eq!(t["year"], 1959);
        assert_eq!(t["missing"], false);
        assert_eq!(t["decodable"], true);
        assert!(t["duration_ms"].as_u64().unwrap() > 0);

        let (status, _) = get_json(&app, "/api/tracks/9999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn fts5_search() {
        let (app, _state, _dir) = scanned_app().await;

        // By artist.
        let (status, v) = get_json(&app, "/api/search?q=Coltrane").await;
        assert_eq!(status, StatusCode::OK);
        assert!(!v.as_array().unwrap().is_empty());
        assert!(v
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["artist"].as_str().unwrap().contains("Coltrane")));

        // By album.
        let (_, v) = get_json(&app, "/api/search?q=Compilation").await;
        assert_eq!(v.as_array().unwrap().len(), 3);

        // Exact title match ranks first.
        let (_, v) = get_json(&app, "/api/search?q=Blue%20Train").await;
        let first = &v.as_array().unwrap()[0];
        assert_eq!(first["album"], "Blue Train");

        // Empty query is a 400, not a full-table dump.
        let (status, _) = get_json(&app, "/api/search?q=").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = get_json(&app, "/api/search?q=%20%20").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn artwork_serving_with_etag() {
        let (app, _state, _dir) = scanned_app().await;
        let (_, v) = get_json(&app, "/api/albums").await;
        let hash = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["title"] == "Blue Train")
            .unwrap()["artwork_hash"]
            .as_str()
            .unwrap()
            .to_string();

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/artwork/{hash}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "image/png");
        let etag = res.headers()[header::ETAG].to_str().unwrap().to_string();
        assert_eq!(etag, format!("\"{hash}\""));
        let body = body_bytes(res).await;
        assert!(!body.is_empty());

        // Conditional request -> 304.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/artwork/{hash}"))
                    .header(header::IF_NONE_MATCH, &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

        // Unknown hash -> 404.
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/artwork/deadbeef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn scan_endpoint_creates_job_and_completes() {
        let (app, state, _dir) = scanned_app().await;

        // Hold the lock manually: the endpoint must report 409.
        let _guard = state.scan_lock.lock().await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        drop(_guard);

        // With the lock free it accepts with 202 and returns the scan job.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        let body = body_bytes(res).await;
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["kind"], "scan");
        let job_id = v["id"].as_str().unwrap().to_string();

        // Poll the job: it must reach done with the scan report in `result`.
        let mut result = String::new();
        for _ in 0..100 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/jobs/{job_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let body = body_bytes(res).await;
            let j: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if j["status"] == "done" {
                result = j["message"].as_str().unwrap_or("").to_string();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(
            result.contains("scan complete:") && result.contains("added"),
            "scan job result: {result}"
        );

        // The background scan ran (a second scan_log row appears).
        let n: i64 = sqlx::query("SELECT COUNT(*) FROM scan_log")
            .fetch_one(&state.pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(n, 2, "background scan did not finish");
    }

    // ------------------------------------------------------------------
    // Phase 3 route tests (S6, S12-transcode, S13)
    // ------------------------------------------------------------------

    /// 16-bit PCM WAV fixture: `frames` sine frames.
    fn wav_fixture(sample_rate: u32, channels: usize, frames: usize) -> Vec<u8> {
        let mut v = Vec::new();
        let data_len = (frames * channels * 2) as u32;
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&(channels as u16).to_le_bytes());
        v.extend_from_slice(&sample_rate.to_le_bytes());
        v.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
        v.extend_from_slice(&((channels * 2) as u16).to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        for f in 0..frames {
            let t = f as f32 / sample_rate as f32;
            let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let q = (s * 32767.0).round() as i16;
            for _ in 0..channels {
                v.extend_from_slice(&q.to_le_bytes());
            }
        }
        v
    }

    /// Minimal stereo DSD64 DSF fixture, `fill`-byte audio blocks.
    fn dsf_fixture(blocks: usize, block_len: usize, fill: u8) -> Vec<u8> {
        fn w32(v: &mut Vec<u8>, x: u32) {
            v.extend_from_slice(&x.to_le_bytes());
        }
        fn w64(v: &mut Vec<u8>, x: u64) {
            v.extend_from_slice(&x.to_le_bytes());
        }
        let mut v = Vec::new();
        v.extend_from_slice(b"DSD ");
        w64(&mut v, 28);
        let size_pos = v.len();
        w64(&mut v, 0);
        let ptr_pos = v.len();
        w64(&mut v, 0);
        v.extend_from_slice(b"fmt ");
        w64(&mut v, 52); // 12-byte header + 40 bytes of fields (Sony spec)
        w32(&mut v, 1);
        w32(&mut v, 0);
        w32(&mut v, 2);
        w32(&mut v, 2);
        w32(&mut v, 2_822_400);
        w32(&mut v, 1);
        w64(&mut v, (blocks * block_len * 8) as u64);
        w32(&mut v, block_len as u32);
        w32(&mut v, 0); // reserved
        let data_off = v.len();
        v.extend_from_slice(b"data");
        w64(&mut v, (12 + blocks * 2 * block_len) as u64);
        for _ in 0..blocks {
            v.extend(std::iter::repeat_n(fill, block_len * 2));
        }
        let total = v.len() as u64;
        v[size_pos..size_pos + 8].copy_from_slice(&total.to_le_bytes());
        v[ptr_pos..ptr_pos + 8].copy_from_slice(&(data_off as u64).to_le_bytes());
        v
    }

    /// Test app with a real audio fixture, a custom config, and a track row
    /// using the given format wire string (e.g. "wav", "dsf").
    async fn audio_app(
        config: ServerConfig,
        fixture: Vec<u8>,
        filename: &str,
        format: &str,
    ) -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let audio_path = dir.path().join(filename);
        std::fs::write(&audio_path, &fixture).unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        let id = db::insert_track_minimal(&pool, audio_path.to_str().unwrap(), "hash1", format)
            .await
            .unwrap();
        assert_eq!(id, 1);
        // S10: the temp dir is the only music root — strict root
        // enforcement stays on in tests.
        let config = ServerConfig {
            music_dirs: vec![dir.path().to_path_buf()],
            ..config
        };
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config,
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        (app(state), dir)
    }

    fn chain_of(res: &axum::response::Response) -> String {
        res.headers()["x-transcode-chain"]
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn passthrough_carries_chain_header() {
        let (app, _state, _dir, _fixture) = test_app(2048).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let chain = chain_of(&res);
        assert!(chain.contains("passthrough"), "chain: {chain}");
    }

    #[tokio::test]
    async fn transcode_flac_response_shape() {
        let (app, _dir) = audio_app(
            ServerConfig::default(),
            wav_fixture(44_100, 2, 44_100),
            "sine.wav",
            "wav",
        )
        .await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=flac")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
        // Live transcode: unknown length, chunked.
        assert!(res.headers().get(header::CONTENT_LENGTH).is_none());
        let chain = chain_of(&res);
        assert!(chain.contains("flac"), "chain: {chain}");
        let body = body_bytes(res).await;
        assert_eq!(&body[..4], b"fLaC");
    }

    #[tokio::test]
    async fn explicit_format_wins_over_ladder() {
        // Ladder prefers opus (disabled in this build) but the explicit
        // ?format=flac must win and succeed.
        let config = ServerConfig {
            preferred_ladder: vec![kahawai_core::StreamFormat::Opus],
            ..Default::default()
        };
        let (app, _dir) = audio_app(config, wav_fixture(44_100, 2, 44_100), "s.wav", "wav").await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=flac")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "explicit flac must win");
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
    }

    #[tokio::test]
    async fn ladder_fallback_names_disabled_feature() {
        // No ?format=: the opus-first ladder resolves to the disabled
        // encoder → 501 naming the cargo feature.
        let config = ServerConfig {
            preferred_ladder: vec![kahawai_core::StreamFormat::Opus],
            ..Default::default()
        };
        let (app, _dir) = audio_app(config, wav_fixture(44_100, 2, 44_100), "s.wav", "wav").await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        #[cfg(not(feature = "encode-opus"))]
        {
            assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
            let body = body_bytes(res).await;
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["feature"], "encode-opus");
        }
        #[cfg(feature = "encode-opus")]
        {
            assert_eq!(res.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn dsd_without_format_defaults_to_flac() {
        let (app, _dir) = audio_app(
            ServerConfig::default(),
            dsf_fixture(48, 4096, 0xFE),
            "tone.dsf",
            "dsf",
        )
        .await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
        let chain = chain_of(&res);
        assert!(chain.starts_with("dsf"), "chain: {chain}");
        let body = body_bytes(res).await;
        assert_eq!(&body[..4], b"fLaC");
    }

    #[tokio::test]
    async fn dsd_native_story_defaults_to_dop() {
        let config = ServerConfig {
            dsd_story: kahawai_core::DsdStory::Native,
            ..Default::default()
        };
        let (app, _dir) = audio_app(config, dsf_fixture(8, 4096, 0xFE), "t.dsf", "dsf").await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/wav");
        assert_eq!(chain_of(&res), "dsf64->dop64");
        let body = body_bytes(res).await;
        // 8 blocks × 4096 B/channel = 262144 bits/channel = 16384 DoP
        // frames; WAV header (44 B) + frames × 2 ch × 3 B.
        assert_eq!(body.len(), 44 + 16384 * 2 * 3);
        assert_eq!(&body[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes([body[24], body[25], body[26], body[27]]),
            176_400
        );
        // Marker alternation on the DC pattern: first two frames.
        assert_eq!(body[44 + 2], 0x05);
        assert_eq!(body[44 + 2 + 6], 0xFA);
    }

    #[tokio::test]
    async fn transcode_seek_ms_shortens_stream() {
        let (app, _dir) = audio_app(
            ServerConfig::default(),
            wav_fixture(44_100, 2, 88_200), // 2 s
            "s.wav",
            "wav",
        )
        .await;
        let full = body_bytes(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/stream/1?format=flac")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=flac&seek_ms=1000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get(header::CONTENT_LENGTH).is_none());
        let chain = chain_of(&res);
        assert!(chain.contains("flac"), "chain: {chain}");
        let seeked = body_bytes(res).await;
        assert_eq!(&seeked[..4], b"fLaC");
        // 1 s seek into a 2 s file: roughly half the bytes, minus the
        // fixed container header (42 B) present in both.
        assert!(
            seeked.len() > full.len() / 2 - 2000 && seeked.len() < full.len(),
            "full={} seeked={}",
            full.len(),
            seeked.len()
        );
    }

    // ------------------------------------------------------------------
    // S5b: native DSD over DoP
    // ------------------------------------------------------------------

    /// Test app with a stereo DSD64 DSF track carrying a non-symmetric LCG
    /// bit pattern (one DSF block group: 32768 bits/channel, no padding).
    /// Returns the app plus the source time-order bits (channel-interleaved).
    async fn dop_app(
        dsd_story: kahawai_core::config::DsdStory,
    ) -> (Router, AppState, tempfile::TempDir, Vec<bool>) {
        let dir = tempfile::tempdir().unwrap();
        let channels = 2usize;
        let per_ch = 32_768usize;
        let bits = crate::dop::fixture::lcg_bits(0xbeef_cafe_1234, channels * per_ch);
        let dsf = crate::dop::fixture::dsf_fixture(channels, 2_822_400, &bits);
        let audio_path = dir.path().join("track01.dsf");
        std::fs::write(&audio_path, &dsf).unwrap();

        let db_path = dir.path().join("test.db");
        let pool = db::open(&db_path).await.unwrap();
        let id = db::insert_track_minimal(&pool, audio_path.to_str().unwrap(), "dop1", "dsf")
            .await
            .unwrap();
        assert_eq!(id, 1);

        let config = ServerConfig {
            dsd_story,
            music_dirs: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config,
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        (app(state.clone()), state, dir, bits)
    }

    /// Verify a DoP WAV body end-to-end: header fields, strict marker
    /// alternation, and bit-exact payload recovery against the source bits.
    fn assert_dop_body(body: &[u8], bits: &[bool], channels: usize, dop_rate: u32) {
        assert_eq!(&body[0..4], b"RIFF");
        assert_eq!(&body[8..12], b"WAVE");
        assert_eq!(&body[12..16], b"fmt ");
        assert_eq!(
            u16::from_le_bytes([body[20], body[21]]),
            1,
            "PCM format tag"
        );
        assert_eq!(
            u16::from_le_bytes([body[22], body[23]]),
            channels as u16,
            "channels"
        );
        assert_eq!(
            u32::from_le_bytes([body[24], body[25], body[26], body[27]]),
            dop_rate,
            "DoP rate"
        );
        assert_eq!(
            u16::from_le_bytes([body[34], body[35]]),
            24,
            "bits per sample"
        );
        assert_eq!(&body[36..40], b"data");
        let data_len = u32::from_le_bytes([body[40], body[41], body[42], body[43]]) as usize;
        assert_eq!(data_len, body.len() - 44, "real data size");
        let riff_len = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as usize;
        assert_eq!(riff_len + 8, body.len(), "real RIFF size");

        let data = &body[44..];
        let frames = data.len() / (channels * 3);
        assert_eq!(frames * channels * 3, data.len());
        for (f, frame) in data.chunks(channels * 3).enumerate() {
            let want_marker = if f % 2 == 0 { 0x05 } else { 0xFA };
            for c in 0..channels {
                let s = &frame[c * 3..c * 3 + 3];
                assert_eq!(s[2], want_marker, "frame {f} ch{c} marker");
                let w = u16::from_le_bytes([s[0], s[1]]);
                for k in 0..16 {
                    let got = (w >> (15 - k)) & 1 == 1;
                    let want = bits[(f * 16 + k) * channels + c];
                    assert_eq!(got, want, "frame {f} ch{c} bit {k}");
                }
            }
        }
    }

    #[tokio::test]
    async fn stream_dop_explicit_format() {
        let (app, _state, _dir, bits) = dop_app(kahawai_core::config::DsdStory::Pcm).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=dop")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/wav");
        assert_eq!(chain_of(&res), "dsf64->dop64");
        let body = body_bytes(res).await;
        // 2048 frames * 2 ch * 3 B + 44 B header.
        assert_eq!(body.len(), 44 + 2048 * 2 * 3);
        assert_dop_body(&body, &bits, 2, 176_400);
    }

    #[tokio::test]
    async fn stream_dop_rejects_non_dsd_source() {
        let (app, _state, _dir, _fixture) = test_app(2048).await; // mp3
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=dop")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn stream_dop_native_story_default() {
        // No explicit format + dsd_story=native → DoP.
        let (app, _state, _dir, bits) = dop_app(kahawai_core::config::DsdStory::Native).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/wav");
        assert_eq!(chain_of(&res), "dsf64->dop64");
        let body = body_bytes(res).await;
        assert_dop_body(&body, &bits, 2, 176_400);
    }

    #[tokio::test]
    async fn stream_dop_pcm_story_default_is_flac() {
        // No explicit format + dsd_story=pcm → DSD→PCM→FLAC (unchanged).
        let (app, _state, _dir, _bits) = dop_app(kahawai_core::config::DsdStory::Pcm).await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
        let chain = chain_of(&res);
        assert!(chain.starts_with("dsf64->flac"), "chain: {chain}");
        let body = body_bytes(res).await;
        assert_eq!(&body[..4], b"fLaC");
    }

    #[tokio::test]
    async fn stream_dop_seek_starts_at_frame_boundary() {
        let (app, _state, _dir, bits) = dop_app(kahawai_core::config::DsdStory::Pcm).await;
        // Full pack for slicing.
        let full = body_bytes(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/stream/1?format=dop")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        // seek_ms=1 → frame floor(1 * 176400 / 1000) = 176 (even → 0x05).
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=dop&seek_ms=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(chain_of(&res), "dsf64->dop64");
        let body = body_bytes(res).await;
        let off = 44 + 176 * 2 * 3;
        assert_eq!(body, &full[off..], "seek must slice the full pack");
        assert_eq!(
            body.len() as u64,
            44 + 2048 * 2 * 3 - off as u64,
            "content length matches the suffix"
        );
        // First frame of the seeked stream keeps marker parity (0x05).
        assert_eq!(body[2], 0x05);
        // ...and carries DSD bits starting at bit 176*16 per channel.
        let w = u16::from_le_bytes([body[0], body[1]]);
        for k in 0..16 {
            let got = (w >> (15 - k)) & 1 == 1;
            assert_eq!(got, bits[(176 * 16 + k) * 2]);
        }
    }

    #[tokio::test]
    async fn head_dop_reports_same_metadata_as_get() {
        let (app, _state, _dir, _bits) = dop_app(kahawai_core::config::DsdStory::Native).await;
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let get_len = get.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .to_string();
        let get_chain = chain_of(&get);
        let get_ct = get.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_string();
        body_bytes(get).await; // drain

        let head = app
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            head.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            get_ct
        );
        assert_eq!(
            head.headers()[header::CONTENT_LENGTH].to_str().unwrap(),
            get_len,
            "HEAD content-length must match GET"
        );
        assert_eq!(chain_of(&head), get_chain);
        assert!(body_bytes(head).await.is_empty());
    }

    // ------------------------------------------------------------------
    // S8 gapless HTTP tests
    // ------------------------------------------------------------------

    /// Unknown ?next= ids are 404 on both GET and HEAD.
    #[tokio::test]
    async fn stream_next_unknown_id_404() {
        let (app, _state, _dir, _fixture) = test_app(2048).await;
        for (method, uri) in [
            ("GET", "/stream/1?next=99999"),
            ("HEAD", "/stream/1?next=99999"),
        ] {
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
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{method} {uri}");
        }
    }

    /// Valid ?next= on a passthrough stream: the client owns the handoff,
    /// the server just advertises the next id.
    #[tokio::test]
    async fn stream_next_valid_passthrough_carries_headers() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        for i in 1..=2 {
            let p = dir.path().join(format!("t{i}.mp3"));
            std::fs::write(&p, b"fake-audio").unwrap();
            db::insert_track_minimal(&pool, p.to_str().unwrap(), &format!("h{i}"), "mp3")
                .await
                .unwrap();
        }
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let app = app(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?next=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["x-gapless-next"].to_str().unwrap(), "2");
        assert!(res.headers().get("x-gapless-mode").is_none());
    }

    /// Two WAV tracks, same spec, ?format=flac&next= → one continuous FLAC
    /// session (single-session mode), decodable to both tracks' samples.
    #[tokio::test]
    async fn stream_next_gapless_flac_single_session() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        for i in 1..=2 {
            let p = dir.path().join(format!("t{i}.wav"));
            std::fs::write(&p, wav_fixture(44_100, 2, 4410)).unwrap();
            db::insert_track_minimal(&pool, p.to_str().unwrap(), &format!("h{i}"), "wav")
                .await
                .unwrap();
        }
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let app = app(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=flac&next=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["x-gapless-next"].to_str().unwrap(), "2");
        assert_eq!(
            res.headers()["x-gapless-mode"].to_str().unwrap(),
            "single-session"
        );
        assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
        let body = body_bytes(res).await;
        // One FLAC stream header for the whole chain.
        assert_eq!(
            body.windows(4).filter(|w| *w == b"fLaC").count(),
            1,
            "single session must emit one FLAC stream"
        );
    }

    /// Mismatched sample rates → chained mode: two FLAC streams back to back.
    #[tokio::test]
    async fn stream_next_gapless_flac_chained_on_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        let specs = [(44_100u32, "t1.wav"), (48_000u32, "t2.wav")];
        for (i, (rate, name)) in specs.iter().enumerate() {
            let p = dir.path().join(name);
            std::fs::write(&p, wav_fixture(*rate, 2, 4410)).unwrap();
            db::insert_track_minimal(&pool, p.to_str().unwrap(), &format!("h{i}"), "wav")
                .await
                .unwrap();
        }
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let app = app(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=flac&next=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["x-gapless-next"].to_str().unwrap(), "2");
        assert_eq!(res.headers()["x-gapless-mode"].to_str().unwrap(), "chained");
        let body = body_bytes(res).await;
        assert_eq!(
            body.windows(4).filter(|w| *w == b"fLaC").count(),
            2,
            "chained mode must emit two FLAC streams"
        );
    }

    /// S8: WAV→FLAC current + already-FLAC next. The next track must join
    /// the gapless chain (forced decode+re-encode) rather than the chain
    /// being silently reduced to the current track alone.
    #[tokio::test]
    async fn stream_next_gapless_flac_next_already_flac() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        // Track 1: WAV (will transcode to FLAC).
        let p1 = dir.path().join("t1.wav");
        std::fs::write(&p1, wav_fixture(44_100, 2, 4410)).unwrap();
        db::insert_track_minimal(&pool, p1.to_str().unwrap(), "h1", "wav")
            .await
            .unwrap();
        // Track 2: already FLAC (encode a simple PCM pattern).
        let p2 = dir.path().join("t2.flac");
        let mut enc = crate::transcode::FlacStreamEncoder::new(44_100, 2).unwrap();
        let mut flac_bytes = enc.header_bytes();
        let pcm: Vec<f32> = (0..4410 * 2)
            .map(|i| (i as f32 * 0.1).sin() * 0.5)
            .collect();
        flac_bytes.extend(enc.push_f32(&pcm).unwrap());
        flac_bytes.extend(enc.finish().unwrap());
        std::fs::write(&p2, &flac_bytes).unwrap();
        db::insert_track_minimal(&pool, p2.to_str().unwrap(), "h2", "flac")
            .await
            .unwrap();

        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let app = app(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1?format=flac&next=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        // The chain was NOT reduced: we get gapless mode (not just the
        // current track with a handoff header and no mode).
        assert_eq!(res.headers()["x-gapless-next"].to_str().unwrap(), "2");
        assert!(
            res.headers().contains_key("x-gapless-mode"),
            "FLAC next must join the chain, not be dropped"
        );
        let body = body_bytes(res).await;
        // Both tracks' audio is in the response (decodable FLAC).
        assert!(
            body.windows(4).any(|w| w == b"fLaC"),
            "response must contain FLAC audio"
        );
    }

    // ------------------------------------------------------------------
    // S10 hardening tests
    // ------------------------------------------------------------------

    /// A track row pointing outside the music roots is 404, not served.
    #[tokio::test]
    async fn stream_path_outside_roots_rejected() {
        let outside = tempfile::tempdir().unwrap();
        let evil = outside.path().join("evil.mp3");
        std::fs::write(&evil, b"fake-audio").unwrap();

        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        let inside = dir.path().join("ok.mp3");
        std::fs::write(&inside, b"fake-audio").unwrap();
        db::insert_track_minimal(&pool, inside.to_str().unwrap(), "h1", "mp3")
            .await
            .unwrap();
        db::insert_track_minimal(&pool, evil.to_str().unwrap(), "h2", "mp3")
            .await
            .unwrap();
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let app = app(state);
        // Track 2's file is outside the roots → 404 (indistinguishable
        // from a missing file, no 403 oracle).
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/stream/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        // Track 1 still serves.
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    /// A symlink inside the music root pointing outside is rejected —
    /// canonicalization resolves the escape.
    #[cfg(unix)]
    #[tokio::test]
    async fn stream_symlink_escape_rejected() {
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.mp3");
        std::fs::write(&target, b"fake-audio").unwrap();

        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link.mp3");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        db::insert_track_minimal(&pool, link.to_str().unwrap(), "h1", "mp3")
            .await
            .unwrap();
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        let app = app(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    /// Non-hex artwork hashes are 400 before any DB lookup.
    #[tokio::test]
    async fn artwork_invalid_hash_400() {
        let (app, _state, _dir, _fixture) = test_app(16).await;
        for uri in ["/api/artwork/not-a-hex-hash!!", "/api/artwork/"] {
            let res = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            // "/api/artwork/" has no hash segment → no route → 404; the
            // non-hex hash itself must be a 400.
            if uri.ends_with('/') {
                assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
            } else {
                assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{uri}");
            }
        }
    }

    /// Request bodies over ~10 MB are rejected with 413 on API routes...
    #[tokio::test]
    async fn api_body_over_limit_rejected() {
        let (app, _state, _dir, _fixture) = test_app(16).await;
        let big = serde_json::json!({ "name": "x".repeat(11 * 1024 * 1024) });
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(big.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// ...while /stream is exempt from the body cap (Range/HEAD on streams
    /// must never trip a body limit).
    #[tokio::test]
    async fn stream_exempt_from_body_limit() {
        let (app, _state, _dir, _fixture) = test_app(2048).await;
        let res = app
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/stream/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    // ------------------------------------------------------------------
    // S7 playlists: append/replace, album expansion, queue save, M3U import
    // ------------------------------------------------------------------

    /// App with 4 mp3 tracks, ids 1..=4, under the temp music root.
    async fn playlist_app() -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        for i in 1..=4 {
            let p = dir.path().join(format!("t{i}.mp3"));
            std::fs::write(&p, b"fake-audio").unwrap();
            let id = db::insert_track_minimal(&pool, p.to_str().unwrap(), &format!("h{i}"), "mp3")
                .await
                .unwrap();
            assert_eq!(id, i);
        }
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        (app(state), dir)
    }

    async fn post_json(
        app: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = body_bytes(res).await;
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    async fn put_json(
        app: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = body_bytes(res).await;
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    /// PUT tracks defaults to append; explicit append adds; replace swaps.
    /// Positions stay dense (0..n).
    #[tokio::test]
    async fn playlist_append_replace_modes() {
        let (app, dir) = playlist_app().await;
        let (status, v) = post_json(
            &app,
            "/api/playlists",
            serde_json::json!({ "name": "mix", "track_ids": [1, 2] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let pid = v["id"].as_i64().unwrap();
        let uri = format!("/api/playlists/{pid}/tracks");
        assert_eq!(v["track_ids"], serde_json::json!([1, 2]));

        // No mode → append is the default.
        let (status, v) = put_json(&app, &uri, serde_json::json!({ "track_ids": [3] })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["track_ids"], serde_json::json!([1, 2, 3]));

        // Explicit append.
        let (status, v) = put_json(
            &app,
            &uri,
            serde_json::json!({ "track_ids": [4], "mode": "append" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["track_ids"], serde_json::json!([1, 2, 3, 4]));

        // Replace wipes the list.
        let (status, v) = put_json(
            &app,
            &uri,
            serde_json::json!({ "track_ids": [2], "mode": "replace" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["track_ids"], serde_json::json!([2]));

        // Positions are dense 0..n in the DB (verified directly; the API
        // has no GET single-playlist route, only the list).
        let pool = crate::db::open(&dir.path().join("test.db")).await.unwrap();
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT position, track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position",
        )
        .bind(pid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows, vec![(0, 2)]);
    }

    /// Album expansion orders by (disc_no, track_no, id), not by the
    /// order ids were given.
    #[tokio::test]
    async fn playlist_album_expansion_orders_disc_track() {
        let (app, dir) = playlist_app().await;
        // Reuse the app's DB via a fresh pool handle on the same file.
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        sqlx::query("INSERT INTO albums (id, title) VALUES (1, 'A')")
            .execute(&pool)
            .await
            .unwrap();
        // (track_id, disc_no, track_no): deliberately shuffled.
        for (tid, disc, no) in [(1, 1, 2), (2, 1, 1), (3, 2, 1), (4, 1, 3)] {
            sqlx::query("UPDATE tracks SET album_id = 1, disc_no = ?, track_no = ? WHERE id = ?")
                .bind(disc)
                .bind(no)
                .bind(tid)
                .execute(&pool)
                .await
                .unwrap();
        }
        let (status, v) = post_json(
            &app,
            "/api/playlists",
            serde_json::json!({ "name": "album mix" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let pid = v["id"].as_i64().unwrap();
        let (status, v) = put_json(
            &app,
            &format!("/api/playlists/{pid}/tracks"),
            serde_json::json!({ "album_ids": [1] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // disc 1: tracks 1,2,3 → ids [2,1,4]; then disc 2 track 1 → id 3.
        assert_eq!(v["track_ids"], serde_json::json!([2, 1, 4, 3]));
    }

    /// from_queue saves the queue's id list in queue order.
    #[tokio::test]
    async fn playlist_create_from_queue() {
        let (app, _dir) = playlist_app().await;
        let (status, v) = post_json(
            &app,
            "/api/playlists",
            serde_json::json!({
                "name": "queue save",
                "from_queue": true,
                "queue_track_ids": [3, 1, 2],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["name"], serde_json::json!("queue save"));
        assert_eq!(v["track_ids"], serde_json::json!([3, 1, 2]));
    }

    /// JSON import: absolute paths, paths relative to a music root,
    /// #EXTINF lines (parsed, not used for matching), and bogus entries
    /// reported in `unmatched` in order.
    #[tokio::test]
    async fn import_playlist_json_paths() {
        let (app, dir) = playlist_app().await;
        let t1 = dir.path().join("t1.mp3");
        let m3u = format!(
            "#EXTM3U\n#EXTINF:200,Artist - One\n{}\n#EXTINF:180,Artist - Two\nt2.mp3\n# a comment\n/nonexistent/ghost.mp3\nnope.mp3\n",
            t1.to_str().unwrap()
        );
        let m3u_path = dir.path().join("list.m3u");
        std::fs::write(&m3u_path, m3u).unwrap();

        let (status, v) = post_json(
            &app,
            "/api/playlists/import",
            serde_json::json!({ "name": "imported", "path": m3u_path.to_str().unwrap() }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["matched"], 2);
        let unmatched = v["unmatched"].as_array().unwrap();
        assert_eq!(unmatched.len(), 2);
        assert_eq!(unmatched[0], serde_json::json!("/nonexistent/ghost.mp3"));
        assert_eq!(unmatched[1], serde_json::json!("nope.mp3"));

        // The playlist exists with the matched tracks in file order
        // (verified via the list endpoint; there is no GET single-playlist).
        let pid = v["playlist_id"].as_i64().unwrap();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/playlists")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_bytes(res).await;
        let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let pl = list
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == pid)
            .expect("imported playlist in list");
        assert_eq!(pl["name"], serde_json::json!("imported"));
        assert_eq!(pl["track_ids"], serde_json::json!([1, 2]));
    }

    /// A server-local playlist path outside the music roots is rejected —
    /// its raw lines must not become a filesystem-read oracle.
    #[tokio::test]
    async fn import_playlist_json_path_outside_roots_rejected() {
        let (app, _dir) = playlist_app().await;
        let outside = tempfile::tempdir().unwrap();
        let m3u_path = outside.path().join("evil.m3u");
        std::fs::write(&m3u_path, "t1.mp3\n").unwrap();
        let (status, _) = post_json(
            &app,
            "/api/playlists/import",
            serde_json::json!({ "path": m3u_path.to_str().unwrap() }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Multipart import: `file` field bytes + optional `name` field.
    #[tokio::test]
    async fn import_playlist_multipart() {
        let (app, _dir) = playlist_app().await;
        let boundary = "----testboundary1234";
        let m3u = "t3.mp3\nt4.mp3\nbogus.mp3\n";
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nMy Upload\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"list.m3u\"\r\n\
             Content-Type: audio/x-mpegurl\r\n\r\n{m3u}\r\n\
             --{b}--\r\n",
            b = boundary
        );
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists/import")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = body_bytes(res).await;
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["matched"], 2);
        assert_eq!(v["unmatched"], serde_json::json!(["bogus.mp3"]));
        let pid = v["playlist_id"].as_i64().unwrap();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/playlists")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_bytes(res).await;
        let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let pl = list
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == pid)
            .expect("imported playlist in list");
        assert_eq!(pl["name"], serde_json::json!("My Upload"));
        assert_eq!(pl["track_ids"], serde_json::json!([3, 4]));
    }

    /// Multipart without a `file` field is a 400.
    #[tokio::test]
    async fn import_playlist_multipart_missing_file_400() {
        let (app, _dir) = playlist_app().await;
        let boundary = "----testboundary1234";
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nNo File\r\n--{b}--\r\n",
            b = boundary
        );
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists/import")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    /// An import body over 10 MB is rejected with 413. Real clients send
    /// Content-Length, which lets the body-cap layer reject without reading.
    #[tokio::test]
    async fn import_playlist_oversized_413() {
        let (app, _dir) = playlist_app().await;
        let boundary = "----testboundary1234";
        let filler = "t1.mp3\n".repeat(2 * 1024 * 1024); // ~14 MB of entries
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"big.m3u\"\r\n\r\n{filler}--{b}--\r\n",
            b = boundary
        );
        assert!(body.len() > 10 * 1024 * 1024);
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists/import")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::CONTENT_LENGTH, body.len().to_string())
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    // ------------------------------------------------------------------
    // S9 job tests (HTTP level)
    // ------------------------------------------------------------------

    /// extract_iso never shells out: missing path → 400, missing file →
    /// 404, existing file → 202 and the job fails honestly, naming the
    /// unimplemented sacd_extract integration.
    #[tokio::test]
    async fn extract_iso_job_fails_honestly() {
        let (app, dir) = playlist_app().await;

        // No path → 400.
        let (status, _) = post_json(
            &app,
            "/api/jobs",
            serde_json::json!({ "kind": "extract_iso" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Missing file → 404.
        let (status, _) = post_json(
            &app,
            "/api/jobs",
            serde_json::json!({ "kind": "extract_iso", "path": "/nonexistent/x.iso" }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Existing file → 202 with the job; it then fails honestly.
        let iso = dir.path().join("disc.iso");
        std::fs::write(&iso, b"fake-iso").unwrap();
        let (status, v) = post_json(
            &app,
            "/api/jobs",
            serde_json::json!({ "kind": "extract_iso", "path": iso.to_str().unwrap() }),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let job_id = v["id"].as_str().unwrap().to_string();
        assert_eq!(v["kind"], serde_json::json!("extract_iso"));
        assert_eq!(
            v["payload"].as_str().unwrap(),
            iso.to_str().unwrap(),
            "the ISO path is persisted as the job payload"
        );

        let mut message = String::new();
        for _ in 0..100 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/jobs/{job_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = body_bytes(res).await;
            let j: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if j["status"] == "failed" {
                message = j["message"].as_str().unwrap_or("").to_string();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            message.contains("sacd_extract"),
            "failure must name the missing integration, got: {message}"
        );
    }

    /// Unknown job ids are 404.
    #[tokio::test]
    async fn get_job_unknown_404() {
        let (app, _dir) = playlist_app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/jobs/job-9999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
