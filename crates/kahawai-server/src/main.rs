//! kahawai-server: self-hosted music streaming server binary.
//! (Spec: kahawai-spec.md)

mod api;
mod audiobooks;
mod audiobooks_api;
mod book_meta;
mod catalog;
mod db;
#[cfg(target_os = "macos")]
mod desktop;
mod dop;
mod dsd;
mod dsd_meta;
mod enrich;
mod export;
mod genre;
mod genre_aliases;
mod hashing;
mod home;
mod jobs;
#[cfg(target_os = "macos")]
mod logfile;
#[cfg(target_os = "macos")]
mod menu;
mod musicbrainz;
mod normalize;
mod podcast_api;
mod podcast_dl;
mod podcast_feed;
mod podcasts;
mod radio;
mod radio_api;
mod resample;
mod scanner;
mod stream;
mod transcode;
mod transcode_cache;

use std::sync::Arc;

use axum::{
    error_handling::HandleErrorLayer,
    http::StatusCode,
    routing::{get, post, put},
    BoxError, Router,
};
use kahawai_core::config::ServerConfig;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing::{info, warn};

/// Pushed to SSE subscribers over `GET /api/events`. `Clone` is required by
/// `broadcast::Sender`; both variants are unit-like so cloning is free.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// A scan finished successfully — the catalog changed.
    CatalogUpdated,
    /// The process is about to exit (SIGINT/SIGTERM received). Sent with a
    /// brief grace period before the graceful shutdown actually closes
    /// connections, so subscribers have a real chance to receive it.
    ShuttingDown,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub jobs: jobs::JobStore,
    /// Shared and mutable so a running server's config can be edited in
    /// place (desktop app: add a music folder, Apply, rescan) without a
    /// restart. `bind` and `db_path` are only ever read at startup — there
    /// is no live path for rebinding the listener or swapping the DB pool.
    pub config: Arc<std::sync::RwLock<ServerConfig>>,
    /// Ensures only one library scan runs at a time (S1).
    pub scan_lock: Arc<tokio::sync::Mutex<()>>,
    /// Ensures only one content-hashing job runs at a time. Separate from
    /// `scan_lock`: hashing can take hours and must not block a rescan.
    pub hash_lock: Arc<tokio::sync::Mutex<()>>,
    /// Held by the one running metadata-enrichment worker. Separate from the
    /// scan and hash locks: enrichment talks to the network, not the disks.
    pub enrich_lock: Arc<tokio::sync::Mutex<()>>,
    /// `GET /api/events` (SSE): lets already-connected clients learn the
    /// catalog changed, or that the server is about to exit, without
    /// polling for either. No receivers is not an error — `send` on an
    /// empty broadcast channel just means nobody's listening.
    pub catalog_events: tokio::sync::broadcast::Sender<ServerEvent>,
    /// Rendered single-track transcodes, served with byte ranges (D3).
    pub transcode_cache: transcode_cache::TranscodeCache,
}

impl AppState {
    pub fn music_dirs(&self) -> Vec<std::path::PathBuf> {
        self.config.read().unwrap().music_dirs.clone()
    }
    /// Where a catalog track's file may live: the music folders, and the
    /// audiobook folders (S10 root enforcement for `/stream`).
    pub async fn track_roots(&self) -> Result<Vec<std::path::PathBuf>, kahawai_core::MusicError> {
        let mut roots = self.music_dirs();
        roots.extend(audiobooks::root_paths(&self.pool).await?);
        Ok(roots)
    }
    pub fn preferred_ladder(&self) -> Vec<kahawai_core::StreamFormat> {
        self.config.read().unwrap().preferred_ladder.clone()
    }
    pub fn dsd_story(&self) -> kahawai_core::DsdStory {
        self.config.read().unwrap().dsd_story
    }
}

/// `POST /api/shutdown` asks the server to stop (see `api::shutdown`). Woken
/// with `notify_waiters`, which leaves nothing behind for a server started
/// later in the same process (the desktop app's restart).
pub(crate) static SHUTDOWN: std::sync::LazyLock<tokio::sync::Notify> =
    std::sync::LazyLock::new(tokio::sync::Notify::new);

/// The last shutdown came from the operating system (SIGINT, SIGTERM), not
/// from the app or the API. The desktop app then exits too: a terminate
/// means the whole program, not just the server inside it.
pub(crate) static SHUTDOWN_BY_SIGNAL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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
        .route("/", get(home::home))
        .route("/api/health", get(api::health))
        .route("/api/identity", get(api::identity))
        .route("/api/shutdown", post(api::shutdown))
        .route("/api/albums", get(api::list_albums))
        .route("/api/albums/{id}", get(api::get_album))
        .route("/api/albums/{id}/export", get(export::export_album))
        .route("/api/artists", get(api::list_artists))
        .route("/api/artists/{id}", get(api::get_artist))
        .route("/api/tracks/{id}", get(api::get_track))
        .route("/api/search", get(api::search))
        .route("/api/catalog", get(catalog::get_catalog))
        .route("/api/catalog/delta", get(catalog::get_catalog_delta))
        .route("/api/genres", get(api::list_genres))
        .route("/api/genres/report", get(api::genre_report))
        .route("/api/genres/{name}/tracks", get(api::genre_tracks))
        .route("/api/enrichment/coverage", get(api::enrichment_coverage))
        .route(
            "/api/playlists",
            get(api::list_playlists).post(api::create_playlist),
        )
        .route("/api/playlists/import", post(api::import_playlist))
        .route(
            "/api/playlists/{id}",
            get(api::get_playlist)
                .delete(api::delete_playlist)
                .patch(api::rename_playlist),
        )
        .route("/api/playlists/{id}/tracks", put(api::set_playlist_tracks))
        .route("/api/playlists/{id}/export", get(export::export_playlist))
        .route("/api/artwork/{hash}", get(api::artwork))
        .route("/api/scan", post(api::trigger_scan))
        .route(
            "/api/audiobook-roots",
            get(audiobooks_api::list_roots).post(audiobooks_api::add_root),
        )
        .route(
            "/api/audiobook-roots/{id}",
            axum::routing::delete(audiobooks_api::delete_root),
        )
        .route(
            "/api/audiobook-listeners",
            get(audiobooks_api::list_listeners).post(audiobooks_api::add_listener),
        )
        .route(
            "/api/audiobook-listeners/{id}",
            axum::routing::delete(audiobooks_api::delete_listener),
        )
        .route(
            "/api/podcasts/feeds",
            get(podcast_api::list_feeds).post(podcast_api::add_feed),
        )
        .route(
            "/api/podcasts/feeds/import-opml",
            post(podcast_api::import_opml),
        )
        .route(
            "/api/podcasts/feeds/export-opml",
            get(podcast_api::export_opml),
        )
        .route(
            "/api/podcasts/feeds/{id}",
            axum::routing::delete(podcast_api::delete_feed),
        )
        .route(
            "/api/podcasts/feeds/{id}/episodes",
            get(podcast_api::list_episodes),
        )
        .route("/api/podcasts/refresh", post(podcast_api::refresh))
        .route("/api/podcasts/folder", get(podcast_api::folder))
        .route(
            "/api/podcasts/feeds/{id}/settings",
            put(podcast_api::put_feed_settings),
        )
        .route(
            "/api/podcasts/episodes/{id}/download",
            post(podcast_api::download).delete(podcast_api::delete_download),
        )
        .route(
            "/api/podcasts/episodes/{id}/file",
            get(podcast_api::episode_file),
        )
        .route(
            "/api/podcasts/episodes/{id}/played",
            post(podcast_api::mark_played),
        )
        .route("/api/radio/search", get(radio_api::search))
        .route("/api/radio/facets/{kind}", get(radio_api::facets))
        .route("/api/radio/probe", post(radio_api::probe))
        .route(
            "/api/radio/favorites",
            get(radio_api::list_favorites).post(radio_api::add_favorite),
        )
        .route(
            "/api/radio/favorites/order",
            put(radio_api::reorder_favorites),
        )
        .route(
            "/api/radio/favorites/{id}",
            axum::routing::patch(radio_api::edit_favorite).delete(radio_api::delete_favorite),
        )
        .route(
            "/api/radio/favorites/{id}/play",
            post(radio_api::play_favorite),
        )
        .route(
            "/api/radio/history",
            get(radio_api::history).post(radio_api::add_history),
        )
        .route("/api/audiobooks", get(audiobooks_api::list_books))
        .route("/api/audiobooks/scan", post(audiobooks_api::trigger_scan))
        .route("/api/audiobooks/enrich", post(audiobooks_api::enrich))
        .route(
            "/api/audiobooks/{id}",
            get(audiobooks_api::book_detail).patch(audiobooks_api::edit_book),
        )
        .route(
            "/api/audiobooks/{id}/position",
            put(audiobooks_api::put_position),
        )
        .route(
            "/api/audiobooks/{id}/finished",
            post(audiobooks_api::mark_finished),
        )
        .route("/api/audiobooks/{id}/history", get(audiobooks_api::history))
        .route("/api/audiobooks/{id}/resolve", get(audiobooks_api::resolve))
        .route(
            "/api/audiobooks/{id}/settings",
            put(audiobooks_api::put_settings),
        )
        .route(
            "/api/audiobooks/{id}/bookmarks",
            get(audiobooks_api::list_bookmarks).post(audiobooks_api::add_bookmark),
        )
        .route(
            "/api/audiobooks/{id}/bookmarks/{bid}",
            axum::routing::patch(audiobooks_api::edit_bookmark)
                .delete(audiobooks_api::delete_bookmark),
        )
        .route("/api/jobs", get(api::list_jobs).post(api::create_job))
        .route("/api/jobs/{id}", get(api::get_job))
        .route("/api/jobs/{id}/pause", post(api::pause_job))
        .route("/api/jobs/{id}/resume", post(api::resume_job))
        .route("/api/jobs/{id}/cancel", post(api::cancel_job))
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
        // Long-lived like the stream routes above: kept outside the `api`
        // sub-router so the 60s API timeout never closes it.
        .route("/api/events", get(api::scan_events))
        .with_state(state)
        // v1 is LAN-only: permissive CORS is acceptable here (spec §3.6).
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

#[cfg(not(target_os = "macos"))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let argv1 = std::env::args().nth(1).map(std::path::PathBuf::from);
    let config_path = ServerConfig::resolve_path(argv1.as_deref())?;
    let config = match ServerConfig::load(&config_path) {
        Ok(c) => {
            info!(path = %config_path.display(), "loaded config");
            c
        }
        Err(e) => {
            warn!(error = %e, path = %config_path.display(), "using default config");
            ServerConfig::default()
        }
    };

    run_server(config).await
}

/// macOS: a Tauri shell instead of a headless process. On launch, an
/// existing usable config starts the server right away in the background
/// (see `desktop::autostart`); otherwise the UI shows the first-run wizard.
/// Linux and Windows are unaffected — this whole function, and every crate
/// it touches, is absent from their dependency graph.
#[cfg(target_os = "macos")]
fn main() {
    // A log file for when the app is opened from Finder (nothing reads its
    // output then). Info and up by default; RUST_LOG overrides.
    let log_path = logfile::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        build = env!("KAHAWAI_GIT_COMMIT"),
        profile = env!("KAHAWAI_PROFILE"),
        log = ?log_path,
        "Kahawai Server starting"
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(desktop::DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            desktop::setup_get_state,
            desktop::setup_pick_directory,
            desktop::setup_validate_dir,
            desktop::setup_save_config,
            desktop::setup_get_running_config,
            desktop::setup_apply_config,
            desktop::setup_audiobook_folders,
            desktop::setup_apply_audiobooks,
            desktop::setup_validate_audiobook_dir,
            desktop::setup_recent_scans,
            desktop::setup_active_hash_job,
            desktop::setup_active_book_lookup,
            desktop::setup_live_scan_stats,
            desktop::setup_enrichment_status,
            desktop::setup_set_enrichment,
            desktop::setup_online_sources,
            desktop::setup_set_online_sources,
            desktop::setup_podcast_settings,
            desktop::setup_set_podcast_settings,
            desktop::setup_enrichment_action,
            desktop::setup_start_server,
            desktop::setup_stop_server,
            desktop::setup_restart_server,
            desktop::setup_server_status,
            desktop::setup_reveal_config,
            desktop::setup_quit,
            desktop::setup_app_ready,
            desktop::splash_shown,
            desktop::setup_reveal_logs,
            desktop::setup_stop_other_server,
            desktop::setup_server_identity,
        ])
        .on_menu_event(menu::on_menu_event)
        // Closing the main window hides it: the server keeps running for the
        // players using it. The Dock icon brings the window back (Reopen,
        // below); Quit (⌘Q, or Quit App) stops everything.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // App menu with a custom About item (the UI shows about.md).
            app.set_menu(menu::build_app_menu(app.handle())?)?;
            desktop::open_windows(app)?;
            desktop::autostart(app);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Kahawai Server desktop shell")
        .run(|app, event| match event {
            tauri::RunEvent::Reopen { .. } => {
                if let Some(main) = tauri::Manager::get_webview_window(app, "main") {
                    let _ = main.show();
                    let _ = main.set_focus();
                }
            }
            // Quitting (⌘Q, Quit App): stop the server gracefully first, so
            // connected players are told and the database closes cleanly.
            tauri::RunEvent::Exit => desktop::stop_server_for_exit(app),
            _ => {}
        });
}

/// Everything after config load: open the catalog, run the startup scan,
/// bind, and serve until a graceful shutdown signal. Both the headless
/// entry point and (on macOS) the desktop shell's spawned task call this.
pub async fn run_server(config: ServerConfig) -> anyhow::Result<()> {
    run_server_with_ready(config, None).await
}

/// Same as [`run_server`], but hands the constructed [`AppState`] back
/// through `ready` as soon as it exists (before binding/serving) — the
/// macOS desktop shell uses this to keep a live handle for its "apply
/// config + rescan" and "recent scans" commands, so they act on the exact
/// same state the HTTP layer does rather than a stale snapshot.
pub async fn run_server_with_ready(
    config: ServerConfig,
    ready: Option<tokio::sync::oneshot::Sender<AppState>>,
) -> anyhow::Result<()> {
    // Bind first: if the port is taken (another Kahawai Server, say a
    // development build), fail before opening the catalog or starting a
    // scan against a database the other server is using.
    let addr: std::net::SocketAddr = config.bind.parse()?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("could not listen on {addr}: {e}"))?;

    api::mark_started();
    let pool = db::open(&config.db_path).await?;
    info!("SQLite catalog open (WAL mode)");

    // S9: durable jobs. Applies the restart rule (in-flight → failed with
    // "server restarted") before serving anything.
    let jobs = jobs::JobStore::persistent(&pool)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    let (catalog_events, _) = tokio::sync::broadcast::channel(16);
    let state = AppState {
        pool,
        jobs,
        config: Arc::new(std::sync::RwLock::new(config.clone())),
        scan_lock: Arc::new(tokio::sync::Mutex::new(())),
        hash_lock: Arc::new(tokio::sync::Mutex::new(())),
        enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
        catalog_events,
        transcode_cache: transcode_cache::TranscodeCache::open(
            config
                .db_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("transcode-cache"),
            config.transcode_cache_mb << 20,
        ),
    };
    if let Some(tx) = ready {
        let _ = tx.send(state.clone());
    }

    // The config's audiobook folders join the catalog's, and a book scan runs
    // unless the startup music scan below is going to run it.
    let audiobook_added = audiobooks::sync_roots(&state.pool, &config.audiobook_dirs)
        .await
        .unwrap_or_else(|e| {
            warn!(error = %e, "could not read the audiobook folders");
            0
        });
    let music_scan_at_start = config.scan_on_startup && !config.music_dirs.is_empty();
    if !music_scan_at_start
        && (audiobook_added > 0
            || !audiobooks::root_paths(&state.pool)
                .await
                .unwrap_or_default()
                .is_empty())
    {
        if let Ok(guard) = state.scan_lock.clone().try_lock_owned() {
            let job = state
                .jobs
                .create(
                    kahawai_core::JobKind::Scan,
                    "Audiobook scan".to_string(),
                    None,
                )
                .await;
            audiobooks_api::spawn_audiobook_scan(state.clone(), job.id, guard);
        }
    }

    // Subscribed podcasts are checked every few hours, new episodes fetched
    // and old played ones cleared (podcast_refresh_hours; 0 turns it off).
    podcast_dl::spawn_scheduler(state.clone());

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
        // Genres come from tags already in the catalog: map them now rather
        // than wait for the next scan (which also does this, first thing).
        let pool = state.pool.clone();
        tokio::spawn(async move {
            if let Err(e) = db::refresh_duplicates(&pool).await {
                warn!(error = %e, "could not refresh duplicate tracks");
            }
            if let Err(e) = genre::refresh_genres(&pool).await {
                warn!(error = %e, "could not refresh genres");
            }
        });
        // Resume content hashing left unfinished by the last run. With a
        // startup scan, the scan queues it when it completes.
        match api::queue_hash_job(&state, "Content hashing").await {
            Ok(Some(job)) => info!(job_id = %job.id, "content hashing resumed"),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "could not queue content hashing"),
        }
    }

    // S10: the LAN-only posture is logged at every startup, not just in docs.
    warn!(
        %addr,
        "LAN-only build: no authentication, no TLS — bind to a trusted network only"
    );
    let events = state.catalog_events.clone();
    // Connection info: `POST /api/shutdown` only listens to this machine.
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(events))
    .await?;
    info!("shutdown complete");
    Ok(())
}

/// SIGINT/SIGTERM → graceful shutdown: in-flight streams drain, then the
/// process exits. Jobs left running/queued in the DB are failed by the S9
/// restart rule on the next boot — never silently resumed.
async fn shutdown_signal(events: tokio::sync::broadcast::Sender<ServerEvent>) {
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
        _ = ctrl_c => SHUTDOWN_BY_SIGNAL.store(true, std::sync::atomic::Ordering::SeqCst),
        _ = terminate => SHUTDOWN_BY_SIGNAL.store(true, std::sync::atomic::Ordering::SeqCst),
        _ = SHUTDOWN.notified() => {},
    }
    info!("shutdown signal received; draining in-flight requests");
    // Tell connected clients (Player apps via SSE) before this function's
    // return lets axum start actually closing connections. No receivers is
    // fine — just means nobody was listening.
    let _ = events.send(ServerEvent::ShuttingDown);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
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
    use kahawai_core::{JobKind, JobStatus};
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
        };
        (app(state.clone()), state, dir)
    }

    async fn body_bytes(res: axum::response::Response) -> Vec<u8> {
        to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    /// `/api/identity`: the Kahawai marker, the version and build, the
    /// library's id (the same one the catalog reports), and this run's start.
    #[tokio::test]
    async fn identity_says_what_and_which_server_this_is() {
        let (app, _state, _dir) = scanned_app().await;
        let (status, v) = get_json(&app, "/api/identity").await;
        assert_eq!(status, StatusCode::OK);
        let id: kahawai_core::ServerIdentity = serde_json::from_value(v).unwrap();
        assert_eq!(id.service, kahawai_core::KAHAWAI_SERVICE);
        assert_eq!(id.name, "Kahawai Server");
        assert_eq!(id.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(id.api_version, 1);
        assert_eq!(
            id.source_url, "https://github.com/ksuayan/kahawai",
            "the AGPL source offer"
        );
        assert!(!id.build.commit.is_empty());
        assert_eq!(id.build.profile, "debug");
        assert!(!id.build.target.is_empty());
        assert!(
            id.build.built_at.ends_with('Z') && id.build.built_at.len() == 20,
            "{}",
            id.build.built_at
        );
        let (_, catalog) = get_json(&app, "/api/catalog").await;
        assert_eq!(id.catalog_id, catalog["catalog_id"]);
        let (_, again) = get_json(&app, "/api/identity").await;
        assert_eq!(again["started_at"], id.started_at, "the same run");
    }

    /// `POST /api/shutdown` stops the server only when asked from this
    /// machine, not from a web page, and with its header.
    #[tokio::test]
    async fn shutdown_only_from_this_machine_not_a_web_page_and_with_the_header() {
        let (app, _state, _dir) = scanned_app().await;
        let call = |peer: &str, origin: Option<&str>, header: Option<&str>| {
            let mut req = Request::builder().method("POST").uri("/api/shutdown");
            if let Some(o) = origin {
                req = req.header(header::ORIGIN, o);
            }
            if let Some(h) = header {
                req = req.header(api::SHUTDOWN_HEADER, h);
            }
            let mut req = req.body(Body::empty()).unwrap();
            req.extensions_mut().insert(axum::extract::ConnectInfo(
                peer.parse::<std::net::SocketAddr>().unwrap(),
            ));
            let app = app.clone();
            async move { app.oneshot(req).await.unwrap().status() }
        };
        assert_eq!(
            call("192.168.1.20:5000", None, Some("yes")).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call("127.0.0.1:5000", Some("https://evil.example"), Some("yes")).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call("127.0.0.1:5000", None, None).await,
            StatusCode::BAD_REQUEST
        );

        let stopped = tokio::spawn(async { SHUTDOWN.notified().await });
        tokio::task::yield_now().await;
        assert_eq!(
            call("127.0.0.1:5000", None, Some("yes")).await,
            StatusCode::ACCEPTED
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), stopped)
            .await
            .expect("the server was told to stop")
            .unwrap();
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

    /// Coverage counts albums, those matched from embedded MusicBrainz IDs,
    /// and those a lookup would still have to find. Albums carry their sort
    /// keys and ID in the API.
    #[tokio::test]
    async fn enrichment_coverage_and_album_fields() {
        let (app, state, _dir) = scanned_app().await;
        let (status, v) = get_json(&app, "/api/enrichment/coverage").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            v,
            serde_json::json!({
                "total_albums": 3,
                "with_embedded_mbid": 0,
                "matched_online": 0,
                "no_match": 0,
                "pending_lookup": 3
            })
        );

        let id: i64 = sqlx::query("SELECT id FROM albums WHERE title = 'Blue Train'")
            .fetch_one(&state.pool)
            .await
            .unwrap()
            .get(0);
        sqlx::query(
            "UPDATE albums SET mbid = '3cc4b4b4-5b0b-4d2d-9d3c-1a9e2f0c4c11',
             enrich_status = 'matched', enrich_source = 'embedded' WHERE id = ?",
        )
        .bind(id)
        .execute(&state.pool)
        .await
        .unwrap();
        let (_, v) = get_json(&app, "/api/enrichment/coverage").await;
        assert_eq!(v["with_embedded_mbid"], 1);
        assert_eq!(v["pending_lookup"], 2);

        let (_, d) = get_json(&app, &format!("/api/albums/{id}")).await;
        assert_eq!(d["album"]["mbid"], "3cc4b4b4-5b0b-4d2d-9d3c-1a9e2f0c4c11");
        assert_eq!(d["album"]["sort_title"], "Blue Train");
        assert_eq!(d["album"]["sort_artist"], "John Coltrane");
        assert_eq!(d["album"]["artwork_source"], "embedded");
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

    /// Phase B: a scan leaves hashes pending and queues the `hash_files` job
    /// itself, which fills them in. It can also be started through
    /// `POST /api/jobs`, one at a time.
    #[tokio::test]
    async fn a_scan_queues_content_hashing_which_fills_every_hash() {
        let (app, state, _dir) = scanned_app().await;
        let pending = || async {
            let n: i64 = sqlx::query("SELECT COUNT(*) FROM tracks WHERE hash IS NULL")
                .fetch_one(&state.pool)
                .await
                .unwrap()
                .get(0);
            n
        };
        assert_eq!(pending().await, 8, "the scan hashes nothing itself");

        let post = |uri: &str, body: &str| {
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let res = app.clone().oneshot(post("/api/scan", "")).await.unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);

        let mut hash_job: Option<serde_json::Value> = None;
        for _ in 0..100 {
            let (_, jobs) = get_json(&app, "/api/jobs").await;
            hash_job = jobs
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["kind"] == "hash_files" && j["status"] == "done")
                .cloned();
            if hash_job.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let hash_job = hash_job.expect("the scan queued a hash_files job that finished");
        assert_eq!(hash_job["message"], "hashed 8 files");
        assert_eq!(pending().await, 0);
        let algos: Vec<String> = sqlx::query("SELECT DISTINCT hash_algo FROM tracks")
            .fetch_all(&state.pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        assert_eq!(algos, vec!["blake3-v1".to_string()]);

        // Manual trigger: one at a time.
        let body = r#"{"kind":"hash_files","label":"Content hashing"}"#;
        let guard = state.hash_lock.lock().await;
        let res = app.clone().oneshot(post("/api/jobs", body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        drop(guard);
        let res = app.clone().oneshot(post("/api/jobs", body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    /// Timings on a copy of a real library, run by hand:
    /// `KAHAWAI_DB_COPY=/path/copy.db cargo test -p kahawai-server
    /// real_db_timings -- --ignored --nocapture`. Opens it the way the server
    /// does (pending migrations, statistics), then times what the player asks for.
    #[tokio::test]
    #[ignore = "needs a copy of a real library database; run by hand"]
    async fn real_db_timings() {
        use std::time::Instant;
        let path = std::env::var("KAHAWAI_DB_COPY").expect("KAHAWAI_DB_COPY");
        let t = Instant::now();
        let pool = db::open(std::path::Path::new(&path)).await.unwrap();
        println!("open (migrations + statistics): {:.2?}", t.elapsed());
        let state = AppState {
            pool: pool.clone(),
            jobs: jobs::JobStore::new(),
            config: Arc::new(std::sync::RwLock::new(ServerConfig::default())),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
        };
        let app = app(state);

        let t = Instant::now();
        let mut page = 1;
        let mut albums = 0;
        loop {
            let (status, v) =
                get_json(&app, &format!("/api/albums?page={page}&per_page=500")).await;
            assert_eq!(status, StatusCode::OK);
            let n = v["items"].as_array().unwrap().len();
            albums += n;
            if n < 500 {
                break;
            }
            page += 1;
        }
        println!(
            "album list, all {page} pages ({albums} albums): {:.2?}",
            t.elapsed()
        );

        let ids: Vec<i64> = sqlx::query("SELECT id FROM albums ORDER BY RANDOM() LIMIT 100")
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        let t = Instant::now();
        for id in &ids {
            let (status, _) = get_json(&app, &format!("/api/albums/{id}")).await;
            assert_eq!(status, StatusCode::OK);
        }
        println!(
            "100 album pages: {:.2?} ({:.1?} each)",
            t.elapsed(),
            t.elapsed() / 100
        );

        let artist: i64 = sqlx::query(
            "SELECT artist_id FROM album_artists GROUP BY artist_id ORDER BY COUNT(*) DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
        let t = Instant::now();
        let (_, v) = get_json(&app, &format!("/api/artists/{artist}")).await;
        println!(
            "busiest artist page ({} albums): {:.2?}",
            v["albums"].as_array().map_or(0, |a| a.len()),
            t.elapsed()
        );

        let t = Instant::now();
        let (_, g) = get_json(&app, "/api/genres").await;
        println!("genre list: {:.2?}", t.elapsed());
        if let Some(name) = g[0]["name"].as_str() {
            let t = Instant::now();
            let (_, v) = get_json(
                &app,
                &format!("/api/genres/{name}/tracks?per_page=200&sort=year&order=desc"),
            )
            .await;
            println!(
                "first 200 of {name} ({} tracks), by year: {:.2?}",
                v["total"],
                t.elapsed()
            );
        }

        let t = Instant::now();
        let snap = catalog::snapshot(&pool).await.unwrap();
        let bytes = serde_json::to_vec(&snap).unwrap().len();
        println!(
            "catalog snapshot: {} tracks, {} albums, {} artists, {:.1} MB JSON: {:.2?}",
            snap.tracks.len(),
            snap.albums.len(),
            snap.artists.len(),
            bytes as f64 / 1e6,
            t.elapsed()
        );
        let t = Instant::now();
        let d = catalog::delta(&pool, Some(&snap.catalog_id), snap.rev)
            .await
            .unwrap();
        println!(
            "empty delta (nothing changed): {:.2?}, empty = {}",
            t.elapsed(),
            d.is_empty()
        );
    }

    /// The same album in two folders (a folder and a backup copy of it):
    /// once hashed, each copy of a track points at the kept one and drops
    /// out of the album, its count, genres, search and the players'
    /// catalog. Removing the kept folder promotes the copies.
    #[tokio::test]
    async fn duplicate_copies_collapse_within_an_album() {
        let (app, state, dir) = scanned_app().await;
        let lib = dir.path().join("lib");
        let album_tracks = |app: Router| async move {
            let (_, albums) = get_json(&app, "/api/albums").await;
            let a = albums["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["title"] == "Blue Train")
                .unwrap()
                .clone();
            let (_, detail) = get_json(&app, &format!("/api/albums/{}", a["id"])).await;
            (
                a["track_count"].as_i64().unwrap(),
                detail["tracks"].as_array().unwrap().len(),
            )
        };
        assert_eq!(album_tracks(app.clone()).await, (3, 3));

        let copy = dir.path().join("lib/Backup/Blue Train");
        std::fs::create_dir_all(&copy).unwrap();
        for f in ["01.flac", "02.flac", "03.wav"] {
            std::fs::copy(lib.join("Blue Train").join(f), copy.join(f)).unwrap();
        }
        crate::scanner::run_scan_with_progress(&state.pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        // Before hashing, nothing says the copies are the same music.
        assert_eq!(album_tracks(app.clone()).await, (6, 6));
        let (_, snap) = get_json(&app, "/api/catalog").await;
        let (id, rev) = (
            snap["catalog_id"].as_str().unwrap().to_string(),
            snap["rev"].as_i64().unwrap(),
        );
        assert_eq!(snap["tracks"].as_array().unwrap().len(), 11);
        crate::hashing::hash_pending(&state.pool, |_, _, _| {})
            .await
            .unwrap();
        assert_eq!(
            db::refresh_duplicates(&state.pool).await.unwrap(),
            3,
            "changed"
        );
        genre::refresh_genres(&state.pool).await.unwrap();
        assert_eq!(album_tracks(app.clone()).await, (3, 3));
        // A second refresh changes nothing.
        let rev_now: i64 = sqlx::query("SELECT value FROM meta WHERE key = 'catalog_rev'")
            .fetch_one(&state.pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(db::refresh_duplicates(&state.pool).await.unwrap(), 0);
        let (_, snap) = get_json(&app, "/api/catalog").await;
        assert_eq!(snap["rev"].as_i64().unwrap(), rev_now);
        assert_eq!(snap["tracks"].as_array().unwrap().len(), 8);
        assert!(snap["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| !t["path"].as_str().unwrap().contains("Backup")));
        let (_, genres) = get_json(&app, "/api/genres").await;
        let (_, found) = get_json(&app, "/api/search?q=Locomotion").await;
        assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
        let jazz = genres
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["name"] == "Jazz")
            .unwrap()
            .clone();
        let (_, before) = get_json(&app, "/api/catalog").await;
        assert_eq!(before["genres"], genres);

        // A player that cached the copies before they were known to be
        // copies: they come back as removed, not as changed tracks.
        let dup_ids: Vec<i64> =
            sqlx::query("SELECT id FROM tracks WHERE duplicate_of IS NOT NULL ORDER BY id")
                .fetch_all(&state.pool)
                .await
                .unwrap()
                .iter()
                .map(|r| r.get(0))
                .collect();
        assert!(dup_ids.iter().all(|&i| i > 8), "the first copies are kept");
        let (_, d) = get_json(
            &app,
            &format!("/api/catalog/delta?since={rev}&catalog_id={id}"),
        )
        .await;
        let d: kahawai_core::CatalogDelta = serde_json::from_value(d).unwrap();
        assert!(!d.full_resync);
        assert!(d.tracks.is_empty(), "{:?}", d.tracks);
        assert_eq!(d.removed_tracks, dup_ids);

        // The kept folder goes: the copies take its place.
        std::fs::remove_dir_all(lib.join("Blue Train")).unwrap();
        crate::scanner::run_scan_with_progress(&state.pool, std::slice::from_ref(&lib), |_, _| {})
            .await
            .unwrap();
        assert_eq!(album_tracks(app.clone()).await, (3, 3));
        let (_, found) = get_json(&app, "/api/search?q=Locomotion").await;
        assert!(
            found[0]["path"].as_str().unwrap().contains("Backup"),
            "{found}"
        );
        let (_, genres) = get_json(&app, "/api/genres").await;
        let jazz_after = genres
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["name"] == "Jazz")
            .unwrap()
            .clone();
        assert_eq!(jazz_after["track_count"], jazz["track_count"]);
    }

    /// The player's catalog: a snapshot with its revision, then deltas that
    /// carry only what changed. A rescan of unchanged files, and fields a
    /// player doesn't show, cost nothing; a delta from another database, a
    /// revision from the future, or a large change asks for a full pull.
    #[tokio::test]
    async fn catalog_snapshot_and_deltas() {
        let (app, state, dir) = scanned_app().await;
        let (status, snap) = get_json(&app, "/api/catalog").await;
        assert_eq!(status, StatusCode::OK);
        let id = snap["catalog_id"].as_str().unwrap().to_string();
        let rev = snap["rev"].as_i64().unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(snap["tracks"].as_array().unwrap().len(), 8);
        assert_eq!(snap["albums"].as_array().unwrap().len(), 3);
        assert!(!snap["artists"].as_array().unwrap().is_empty());
        assert_eq!(snap["genres"][0]["name"], "Jazz");

        let delta = |since: i64, id: &str| {
            let (app, uri) = (
                app.clone(),
                format!("/api/catalog/delta?since={since}&catalog_id={id}"),
            );
            async move {
                let (status, v) = get_json(&app, &uri).await;
                assert_eq!(status, StatusCode::OK);
                serde_json::from_value::<kahawai_core::CatalogDelta>(v).unwrap()
            }
        };
        let d = delta(rev, &id).await;
        assert!(d.is_empty() && d.rev == rev, "{d:?}");
        assert_eq!(d.genres.len(), 1, "the genre list always comes whole");

        // Rescanning unchanged files changes nothing a player shows.
        let lib = dir.path().join("lib");
        crate::scanner::run_scan_with_progress(&state.pool, &[lib], |_, _| {})
            .await
            .unwrap();
        assert!(delta(rev, &id).await.is_empty(), "a no-op rescan is free");

        // A content hash isn't shown; a title and a missing flag are.
        let ids: Vec<i64> = sqlx::query("SELECT id FROM tracks ORDER BY id")
            .fetch_all(&state.pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        for sql in [
            "UPDATE tracks SET hash = 'feed' WHERE id = ?",
            "UPDATE tracks SET title = 'Renamed' WHERE id = ?",
        ] {
            sqlx::query(sql)
                .bind(ids[0])
                .execute(&state.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE tracks SET missing = 1 WHERE id = ?")
            .bind(ids[1])
            .execute(&state.pool)
            .await
            .unwrap();
        let d = delta(rev, &id).await;
        assert!(d.rev > rev, "revisions only go up");
        let changed: Vec<(i64, Option<String>, bool)> = d
            .tracks
            .iter()
            .map(|t| (t.id, t.title.clone(), t.missing))
            .collect();
        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0].1.as_deref(), Some("Renamed"));
        assert_eq!((changed[1].0, changed[1].2), (ids[1], true), "went missing");
        assert!(d.albums.is_empty() && d.artists.is_empty());

        // From here on, only what came after.
        let rev2 = d.rev;
        let album: i64 = sqlx::query("SELECT id FROM albums ORDER BY id LIMIT 1")
            .fetch_one(&state.pool)
            .await
            .unwrap()
            .get(0);
        sqlx::query("UPDATE albums SET year = 1961 WHERE id = ?")
            .bind(album)
            .execute(&state.pool)
            .await
            .unwrap();
        let d = delta(rev2, &id).await;
        assert!(d.tracks.is_empty());
        assert_eq!(d.albums.len(), 1);
        assert_eq!(d.albums[0].year, Some(1961));

        // A deleted album (duplicates merged) leaves a tombstone.
        let rev3 = d.rev;
        sqlx::query("UPDATE tracks SET album_id = NULL WHERE album_id = ?")
            .bind(album)
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM album_artists WHERE album_id = ?")
            .bind(album)
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM albums WHERE id = ?")
            .bind(album)
            .execute(&state.pool)
            .await
            .unwrap();
        let d = delta(rev3, &id).await;
        assert_eq!(d.removed_albums, vec![album]);

        // Can't patch: another database, a future revision, too much changed.
        assert!(delta(rev, "someotherdatabase").await.full_resync);
        assert!(delta(d.rev + 100, &id).await.full_resync);
        sqlx::query("UPDATE tracks SET title = title || '!'")
            .execute(&state.pool)
            .await
            .unwrap();
        assert!(delta(d.rev, &id).await.full_resync, "8 of ~14 rows changed");
    }

    /// A scan maps raw genre tags to canonical genres; the genre endpoints
    /// count present tracks only, page them, and report what the alias table
    /// didn't cover; text search matches the genre tag.
    #[tokio::test]
    async fn genres_browse_report_and_search() {
        let (app, state, _dir) = scanned_app().await;
        let (status, v) = get_json(&app, "/api/genres").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v, serde_json::json!([{ "name": "Jazz", "track_count": 8 }]));
        let (_, hits) = get_json(&app, "/api/search?q=jazz").await;
        assert_eq!(hits.as_array().unwrap().len(), 8, "genre is searchable");

        let ids: Vec<i64> = sqlx::query("SELECT id FROM tracks ORDER BY id")
            .fetch_all(&state.pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        for (id, genre) in [
            (ids[0], Some("Pop, Rock")),
            (ids[1], Some("Pinoy Rock")),
            (ids[2], Some("Unknown genre")),
            (ids[3], None),
        ] {
            sqlx::query("UPDATE tracks SET genre = ? WHERE id = ?")
                .bind(genre)
                .bind(id)
                .execute(&state.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE tracks SET missing = 1 WHERE id = ?")
            .bind(ids[4])
            .execute(&state.pool)
            .await
            .unwrap();
        genre::refresh_genres(&state.pool).await.unwrap();

        let (_, v) = get_json(&app, "/api/genres").await;
        assert_eq!(
            v,
            serde_json::json!([
                { "name": "Jazz", "track_count": 3 },
                { "name": "Rock", "track_count": 2 },
                { "name": "Pop", "track_count": 1 },
            ]),
            "a missing track isn't counted"
        );

        let (status, v) = get_json(&app, "/api/genres/Rock/tracks?per_page=1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (v["total"].as_u64(), v["items"].as_array().unwrap().len()),
            (Some(2), 1)
        );
        let (status, v) = get_json(&app, "/api/genres/Rock/tracks?page=2&per_page=1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        let (status, _) = get_json(&app, "/api/genres/Polka/tracks").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Sorted on the server, so pages stay in order.
        let albums = |v: &serde_json::Value| -> Vec<String> {
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["album"].as_str().unwrap_or_default().to_string())
                .collect()
        };
        let (_, asc) = get_json(&app, "/api/genres/Jazz/tracks?sort=title").await;
        let (_, desc) = get_json(&app, "/api/genres/Jazz/tracks?sort=title&order=desc").await;
        let (a, d) = (albums(&asc), albums(&desc));
        assert!(
            a.windows(2)
                .all(|w| w[0].to_lowercase() <= w[1].to_lowercase()),
            "{a:?}"
        );
        assert!(
            d.windows(2)
                .all(|w| w[0].to_lowercase() >= w[1].to_lowercase()),
            "{d:?}"
        );
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/genres/Jazz/tracks?sort=bogus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let (_, r) = get_json(&app, "/api/genres/report").await;
        assert_eq!(r["distinct_raw"], 4);
        assert_eq!(
            r["by_keyword"],
            serde_json::json!([{ "raw": "Pinoy Rock", "track_count": 1, "genres": ["Rock"] }])
        );
        assert_eq!(
            r["ignored"],
            serde_json::json!([{ "raw": "Unknown genre", "track_count": 1, "genres": [] }])
        );
        assert_eq!(r["unmapped"], serde_json::json!([]));
    }

    /// Online lookup is opt-in and runs one at a time; pause, resume and
    /// cancel move it between states and refuse every other kind of job.
    /// (The enrich_lock is held throughout so no worker reaches the real
    /// MusicBrainz: as if one were running, paused in place.)
    #[tokio::test]
    async fn enrich_job_is_opt_in_and_can_be_paused_resumed_and_cancelled() {
        let (app, state, _dir) = scanned_app().await;
        let start = serde_json::json!({"kind": "enrich_metadata", "label": "Album info lookup"});
        let (status, v) = post_json(&app, "/api/jobs", start.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "off by default");
        assert!(
            v["error"].as_str().unwrap_or_default().contains("Settings"),
            "{v}"
        );

        state.config.write().unwrap().enrichment_enabled = true;
        let _worker = state.enrich_lock.lock().await;
        let (status, _) = post_json(&app, "/api/jobs", start).await;
        assert_eq!(status, StatusCode::CONFLICT, "one at a time");

        let hash = state
            .jobs
            .create(JobKind::HashFiles, "Content hashing".into(), None)
            .await;
        for action in ["pause", "resume", "cancel"] {
            let (status, _) = post_json(
                &app,
                &format!("/api/jobs/{}/{action}", hash.id),
                serde_json::json!({}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{action} a hash job");
        }
        let (status, _) = post_json(&app, "/api/jobs/nope/pause", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let job = state
            .jobs
            .create(JobKind::EnrichMetadata, "Album info lookup".into(), None)
            .await;
        state.jobs.set_status(&job.id, JobStatus::Running).await;
        let act = |action: &str| {
            let (app, uri) = (app.clone(), format!("/api/jobs/{}/{action}", job.id));
            async move { post_json(&app, &uri, serde_json::json!({})).await }
        };
        let (status, v) = act("pause").await;
        assert_eq!(
            (status, v["status"].as_str()),
            (StatusCode::OK, Some("paused"))
        );
        assert_eq!(act("pause").await.0, StatusCode::CONFLICT);

        // A scan finishing now doesn't queue a second lookup behind the
        // paused one: that one is the user's to resume.
        api::maybe_queue_enrich(&state).await.unwrap();
        let lookups = state
            .jobs
            .list()
            .into_iter()
            .filter(|j| j.kind == JobKind::EnrichMetadata)
            .count();
        assert_eq!(lookups, 1);

        state
            .jobs
            .set_message(&job.id, Some("Paused: offline".into()))
            .await;
        let (status, v) = act("resume").await;
        assert_eq!(
            (status, v["status"].as_str()),
            (StatusCode::OK, Some("running"))
        );
        assert!(v["message"].is_null(), "the pause reason is cleared");
        assert_eq!(act("resume").await.0, StatusCode::CONFLICT);

        let (status, v) = act("cancel").await;
        assert_eq!(
            (status, v["status"].as_str()),
            (StatusCode::OK, Some("cancelled"))
        );
        for action in ["pause", "resume", "cancel"] {
            assert_eq!(
                act(action).await.0,
                StatusCode::CONFLICT,
                "{action} after cancel"
            );
        }

        // Settings: the threshold is range-checked, and turning lookup off
        // cancels one in progress (no names leave the LAN once it's off).
        assert!(matches!(
            api::set_enrichment(&state, true, 0.3).await,
            Err(kahawai_core::MusicError::BadRequest(_))
        ));
        let job = state
            .jobs
            .create(JobKind::EnrichMetadata, "Album info lookup".into(), None)
            .await;
        state.jobs.set_status(&job.id, JobStatus::Paused).await;
        api::set_enrichment(&state, false, 0.8).await.unwrap();
        assert_eq!(
            state.jobs.get(&job.id).unwrap().status,
            JobStatus::Cancelled
        );
        assert_eq!(state.config.read().unwrap().enrichment_min_confidence, 0.8);

        // Lowering the threshold puts "not found" albums back in line (a
        // looser match may now succeed); raising it doesn't; a matched album
        // stays matched.
        let count = |status: &'static str| {
            let pool = state.pool.clone();
            async move {
                sqlx::query("SELECT COUNT(*) FROM albums WHERE enrich_status = ?")
                    .bind(status)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .get::<i64, _>(0)
            }
        };
        sqlx::query("UPDATE albums SET enrich_status = 'no_match', enrich_attempts = 1")
            .execute(&state.pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE albums SET enrich_status = 'matched', mbid = 'x'
             WHERE id = (SELECT MIN(id) FROM albums)",
        )
        .execute(&state.pool)
        .await
        .unwrap();
        api::set_enrichment(&state, false, 0.95).await.unwrap();
        assert_eq!(count("no_match").await, 2, "stricter: nothing to retry");
        api::set_enrichment(&state, false, 0.8).await.unwrap();
        assert_eq!((count("pending").await, count("no_match").await), (2, 0));
        assert_eq!(count("matched").await, 1);
        let attempts: i64 =
            sqlx::query("SELECT MAX(enrich_attempts) FROM albums WHERE enrich_status = 'pending'")
                .fetch_one(&state.pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(attempts, 0);
    }

    /// A taken port fails the start before anything else happens: no
    /// catalog opened, no startup scan against a database another server
    /// (say a development build) is using.
    #[tokio::test]
    async fn a_taken_port_fails_before_the_catalog_is_opened() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("music.db");
        let config = ServerConfig {
            bind: taken.local_addr().unwrap().to_string(),
            db_path: db_path.clone(),
            music_dirs: vec![dir.path().to_path_buf()],
            scan_on_startup: true,
            ..Default::default()
        };
        let err = run_server_with_ready(config, None).await.unwrap_err();
        assert!(err.to_string().contains("could not listen on"), "{err}");
        assert!(!db_path.exists(), "the catalog was never opened");
    }

    /// The desktop shell edits `AppState.config` in place (add a music
    /// folder, Apply) rather than restarting the process; this is the
    /// plumbing that makes that possible.
    #[tokio::test]
    async fn app_state_music_dirs_reflects_a_live_config_edit() {
        let (_app, state, _dir) = scanned_app().await;
        let original = state.music_dirs();
        let new_dirs = vec![std::path::PathBuf::from("/tmp/a-different-music-folder")];
        state.config.write().unwrap().music_dirs = new_dirs.clone();
        assert_ne!(state.music_dirs(), original);
        assert_eq!(state.music_dirs(), new_dirs);
    }

    /// A scan started from anywhere (HTTP or, in the desktop shell, an
    /// internal command) broadcasts on completion, so a Player that already
    /// has an open `/api/events` connection learns the catalog changed
    /// without needing to be the one that triggered — or even be aware of —
    /// the scan.
    #[tokio::test]
    async fn scan_completion_broadcasts_a_catalog_updated_event() {
        use tokio_stream::StreamExt;

        let (app, _state, _dir) = scanned_app().await;

        let sse_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sse_res.status(), StatusCode::OK);
        let mut events = sse_res.into_body().into_data_stream();

        let scan_res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/scan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(scan_res.status(), StatusCode::ACCEPTED);

        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = events.next().await.expect("SSE stream ended").unwrap();
                if !bytes.is_empty() {
                    return bytes;
                }
            }
        })
        .await
        .expect("timed out waiting for a catalog-updated SSE event");
        assert!(
            String::from_utf8_lossy(&chunk).contains("catalog-updated"),
            "unexpected SSE payload: {:?}",
            String::from_utf8_lossy(&chunk)
        );
    }

    /// `shutdown_signal` sends this on the same channel before the graceful
    /// shutdown starts closing connections — this locks in that a
    /// `ShuttingDown` event renders as `server-shutting-down` for
    /// subscribers, independent of exercising the actual signal handler.
    #[tokio::test]
    async fn shutting_down_event_renders_as_server_shutting_down() {
        use tokio_stream::StreamExt;

        let (app, state, _dir) = scanned_app().await;

        let sse_res = app
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut events = sse_res.into_body().into_data_stream();

        let _ = state.catalog_events.send(ServerEvent::ShuttingDown);

        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let bytes = events.next().await.expect("SSE stream ended").unwrap();
                if !bytes.is_empty() {
                    return bytes;
                }
            }
        })
        .await
        .expect("timed out waiting for the server-shutting-down SSE event");
        assert!(
            String::from_utf8_lossy(&chunk).contains("server-shutting-down"),
            "unexpected SSE payload: {:?}",
            String::from_utf8_lossy(&chunk)
        );
    }

    // ------------------------------------------------------------------
    // Phase 3 route tests (S6, S12-transcode, S13)
    // ------------------------------------------------------------------

    /// 16-bit PCM WAV fixture: `frames` sine frames.
    pub(crate) fn wav_fixture(sample_rate: u32, channels: usize, frames: usize) -> Vec<u8> {
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
    pub(crate) fn dsf_fixture(blocks: usize, block_len: usize, fill: u8) -> Vec<u8> {
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
            config: Arc::new(std::sync::RwLock::new(config)),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(config)),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![dir.path().to_path_buf()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: transcode_cache::TranscodeCache::disabled(),
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

    /// GET a single playlist by id returns its name and ordered track ids;
    /// a nonexistent id 404s.
    #[tokio::test]
    async fn get_playlist_returns_tracks_or_404() {
        let (app, _dir) = playlist_app().await;
        let (status, v) = post_json(
            &app,
            "/api/playlists",
            serde_json::json!({ "name": "Chill Bill", "track_ids": [2, 1, 3] }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let pid = v["id"].as_i64().unwrap();

        let (status, v) = get_json(&app, &format!("/api/playlists/{pid}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["name"], serde_json::json!("Chill Bill"));
        assert_eq!(v["track_ids"], serde_json::json!([2, 1, 3]));

        let (status, _) = get_json(&app, "/api/playlists/9999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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

        // Positions are dense 0..n in the DB — checked directly, since
        // track_ids order alone wouldn't rule out sparse positions.
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

        // The playlist exists with the matched tracks in file order.
        let pid = v["playlist_id"].as_i64().unwrap();
        let (status, pl) = get_json(&app, &format!("/api/playlists/{pid}")).await;
        assert_eq!(status, StatusCode::OK);
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
