//! macOS desktop shell: the first-run setup wizard and, on later launches,
//! a minimal status view. See kahawai-server-desktop-ui-spec.md.
//!
//! Absent on every other target — `main.rs` never references this module
//! outside `#[cfg(target_os = "macos")]`, and its own Tauri dependencies
//! are target-gated in Cargo.toml, so it never enters the build graph
//! elsewhere.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kahawai_core::config::ServerConfig;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

/// Shared with every command via `app.manage(...)`. Cheap to clone (every
/// field is an `Arc`), so a clone can be moved into a spawned task while the
/// original stays with Tauri's managed state.
#[derive(Clone, Default)]
pub struct DesktopState {
    server_task: Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    last_error: Arc<Mutex<Option<String>>>,
    bind: Arc<Mutex<Option<String>>>,
    /// The running server's own `AppState`, captured via `run_server`'s
    /// `ready` hook as soon as it's constructed. `None` before the first
    /// successful (or even attempted) start. Commands that need to act on
    /// the *live* server — apply a config edit, trigger a rescan, list
    /// recent scan jobs — go through this rather than the config file, so
    /// they see and affect the exact same state the HTTP layer does.
    app_state: Arc<Mutex<Option<crate::AppState>>>,
}

#[derive(Serialize)]
pub struct SetupState {
    config_path: String,
    config_exists: bool,
    config: Option<ServerConfig>,
}

/// Time budget for `setup_validate_dir`'s walk. Just a UI sanity check (does
/// this folder have music in it?), not the real scan —
/// `scanner::run_scan_with_progress` walks every file, no limit of any kind.
/// Time-boxed rather than count-capped: a real library can be huge (six
/// figures of files isn't unusual), and a fixed file-count cap either
/// truncates almost immediately for a library that size or does nothing
/// useful for a small one. A time budget scales itself — a fast local drive
/// gets an exact count regardless of how many files that takes, a slow
/// network share just reports what it found in the budget and says so.
const VALIDATE_DIR_TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);
/// How often to check the clock — `Instant::now()` per file is wasteful at
/// six-figure file counts.
const VALIDATE_DIR_CLOCK_CHECK_INTERVAL: u32 = 256;

#[derive(Serialize)]
pub struct DirValidation {
    exists: bool,
    is_dir: bool,
    readable: bool,
    writable: bool,
    audio_files: usize,
    /// True if the walk hit `VALIDATE_DIR_FILE_CAP` before finishing —
    /// `audio_files` is then a lower bound, not the true count. Never true
    /// for the actual scan, only this quick preview.
    truncated: bool,
}

#[derive(Deserialize)]
pub struct SetupInput {
    music_dirs: Vec<String>,
    db_dir: String,
    bind: String,
}

#[derive(Serialize, Clone)]
pub struct ServerStatus {
    running: bool,
    bind: String,
}

#[tauri::command]
pub fn setup_get_state() -> SetupState {
    let path = ServerConfig::resolve_path(None).unwrap_or_else(|_| PathBuf::from("config.toml"));
    match ServerConfig::load(&path) {
        Ok(config) => SetupState {
            config_path: path.display().to_string(),
            config_exists: true,
            config: Some(config),
        },
        Err(_) => SetupState {
            config_path: path.display().to_string(),
            config_exists: false,
            config: None,
        },
    }
}

#[tauri::command]
pub async fn setup_pick_directory(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    rx.await
        .ok()
        .flatten()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.display().to_string())
}

/// Create-then-remove a probe file: the only reliable way to check
/// writability without relying on Unix permission bits (network shares,
/// ACLs).
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".kahawai-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[tauri::command]
pub fn setup_validate_dir(path: String) -> DirValidation {
    let dir = Path::new(&path);
    let exists = dir.exists();
    let is_dir = dir.is_dir();
    if !exists || !is_dir {
        return DirValidation {
            exists,
            is_dir,
            readable: false,
            writable: false,
            audio_files: 0,
            truncated: false,
        };
    }
    let readable = std::fs::read_dir(dir).is_ok();
    let writable = is_writable(dir);
    let mut audio_files = 0usize;
    let mut truncated = false;
    if readable {
        let start = std::time::Instant::now();
        let mut checked = 0u32;
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if crate::scanner::is_audio(entry.path()) {
                audio_files += 1;
            }
            checked += 1;
            if checked.is_multiple_of(VALIDATE_DIR_CLOCK_CHECK_INTERVAL)
                && start.elapsed() >= VALIDATE_DIR_TIME_BUDGET
            {
                truncated = true;
                break;
            }
        }
    }
    DirValidation {
        exists,
        is_dir,
        readable,
        writable,
        audio_files,
        truncated,
    }
}

#[tauri::command]
pub fn setup_save_config(input: SetupInput) -> Result<(), String> {
    if input.music_dirs.is_empty() {
        return Err("Add at least one music folder.".to_string());
    }
    let db_dir = PathBuf::from(&input.db_dir);
    if !is_writable(&db_dir) {
        return Err(format!("'{}' is not writable.", input.db_dir));
    }
    let bind: std::net::SocketAddr = input.bind.parse().map_err(|_| {
        format!(
            "'{}' is not a valid address (expected host:port).",
            input.bind
        )
    })?;

    let config = ServerConfig {
        music_dirs: input.music_dirs.into_iter().map(PathBuf::from).collect(),
        bind: bind.to_string(),
        db_path: db_dir.join("music.db"),
        // `ServerConfig::default()` leaves this off (a sane default for a
        // headless install someone hand-configures), but the wizard's whole
        // point is "start the server and see your music" — without this,
        // a fresh install (an empty, never-scanned database) starts serving
        // an empty catalog forever, since nothing else ever triggers a scan.
        scan_on_startup: true,
        ..ServerConfig::default()
    };
    let path = ServerConfig::resolve_path(None).map_err(|e| e.to_string())?;
    config.save(&path).map_err(|e| e.to_string())
}

/// The live config of the currently-running server (not the on-disk file —
/// see `setup_get_state` for that, used before a server exists). Backs the
/// Status view's "view/edit current config" panel. `None` means no server
/// is running yet.
#[tauri::command]
pub fn setup_get_running_config(state: tauri::State<DesktopState>) -> Option<ServerConfig> {
    let app_state = state.app_state.lock().unwrap().clone()?;
    let config = app_state.config.read().unwrap().clone();
    Some(config)
}

#[derive(Deserialize)]
pub struct ApplyConfigInput {
    /// Folders to add. A path already present is a no-op, not a duplicate.
    #[serde(default)]
    add: Vec<String>,
    /// Folders to remove — the *only* way a folder leaves `music_dirs`.
    #[serde(default)]
    remove: Vec<String>,
}

/// Add/remove music folders on a *running* server, no restart: updates the
/// live config in place, persists the same change to disk (so a future
/// restart keeps it), then triggers a rescan of the updated directory list.
/// Deliberately scoped to `music_dirs` only — `bind`/`db_path` can't be
/// hot-swapped safely (the listener is already bound; the DB pool is
/// already open against the old path), so those still need the wizard's
/// save-and-restart path.
///
/// Takes `add`/`remove` deltas, not a full replacement list: the frontend
/// used to send its entire working copy of the folder list, and a bug that
/// left that copy incomplete (e.g. not yet loaded from the running config)
/// silently dropped every folder it didn't know about. Computing the new
/// list here, against the live config, means a folder can only ever
/// disappear because this call explicitly named it in `remove`.
#[tauri::command]
pub async fn setup_apply_config(
    input: ApplyConfigInput,
    state: tauri::State<'_, DesktopState>,
) -> Result<(), String> {
    let app_state = state
        .app_state
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "the server is not running".to_string())?;

    let dirs = {
        let mut cfg = app_state.config.write().unwrap();
        let remove: Vec<PathBuf> = input.remove.iter().map(PathBuf::from).collect();
        cfg.music_dirs.retain(|d| !remove.contains(d));
        for a in input.add {
            let p = PathBuf::from(a);
            if !cfg.music_dirs.contains(&p) {
                cfg.music_dirs.push(p);
            }
        }
        if cfg.music_dirs.is_empty() {
            return Err("At least one music folder is required.".to_string());
        }
        cfg.music_dirs.clone()
    };

    let path = ServerConfig::resolve_path(None).map_err(|e| e.to_string())?;
    let mut on_disk = ServerConfig::load(&path).unwrap_or_default();
    on_disk.music_dirs = dirs;
    on_disk.save(&path).map_err(|e| e.to_string())?;

    // Same path `POST /api/scan` uses: locking, progress, and — on success —
    // the `catalog_events` broadcast that tells already-connected Player
    // clients the catalog changed, without them having to poll for it.
    let guard = app_state
        .scan_lock
        .clone()
        .try_lock_owned()
        .map_err(|_| "a scan is already running".to_string())?;
    let job = app_state
        .jobs
        .create(
            kahawai_core::JobKind::Scan,
            "Library scan".to_string(),
            None,
        )
        .await;
    crate::api::spawn_scan_job(app_state, job, guard);
    Ok(())
}

#[derive(Serialize, Default)]
pub struct LiveScanStats {
    albums: i64,
    artists: i64,
    tracks: i64,
    /// The most recently cataloged album (by insertion order), so the
    /// Status view can show "scanning… last added: <album>" — a per-album
    /// sense of progress without the scanner needing its own event stream.
    last_album: Option<String>,
    last_album_artist: Option<String>,
}

/// Live catalog counts, polled by the Status view while a scan is running.
/// Reads straight from the DB rather than the scanner's own file-level
/// progress, so it reflects exactly what's actually been committed so far.
#[tauri::command]
pub async fn setup_live_scan_stats(
    state: tauri::State<'_, DesktopState>,
) -> Result<LiveScanStats, String> {
    let Some(app_state) = state.app_state.lock().unwrap().clone() else {
        return Ok(LiveScanStats::default());
    };
    let pool = &app_state.pool;
    let albums: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM albums")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    let artists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artists")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    let tracks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    let last: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT title, artist FROM albums ORDER BY id DESC LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
    Ok(LiveScanStats {
        albums,
        artists,
        tracks,
        last_album: last.as_ref().map(|(t, _)| t.clone()),
        last_album_artist: last.and_then(|(_, a)| a),
    })
}

/// The most recent scan jobs (successes and failures alike), for the Status
/// view's "recent scans" list. `Job.status`/`Job.message` already carry
/// everything needed — a failed scan's honest error, verbatim — so this
/// reuses the existing job store rather than adding new state to track.
#[tauri::command]
pub fn setup_recent_scans(state: tauri::State<DesktopState>) -> Vec<kahawai_core::Job> {
    let Some(app_state) = state.app_state.lock().unwrap().clone() else {
        return Vec::new();
    };
    let mut jobs: Vec<kahawai_core::Job> = app_state
        .jobs
        .list()
        .into_iter()
        .filter(|j| j.kind == kahawai_core::JobKind::Scan)
        .collect();
    // Job ids are sequential ("job-0001", "job-0002", …) — sorting by id
    // descending is newest-first without needing a separate timestamp.
    jobs.sort_by(|a, b| b.id.cmp(&a.id));
    jobs.truncate(10);
    jobs
}

/// Settings → Album info: whether online lookup is on, its threshold, how
/// much of the library it covers, and the latest lookup job.
#[derive(Serialize)]
pub struct EnrichmentStatus {
    enabled: bool,
    min_confidence: f32,
    coverage: crate::api::EnrichmentCoverage,
    job: Option<kahawai_core::Job>,
}

/// A server error as the Settings tab shows it: the message alone for the
/// ones written for the user.
fn user_msg(e: kahawai_core::MusicError) -> String {
    use kahawai_core::MusicError::*;
    match e {
        Conflict(m) | BadRequest(m) => m,
        other => other.to_string(),
    }
}

fn live_state(state: &DesktopState) -> Result<crate::AppState, String> {
    state
        .app_state
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "the server is not running".to_string())
}

#[tauri::command]
pub async fn setup_enrichment_status(
    state: tauri::State<'_, DesktopState>,
) -> Result<EnrichmentStatus, String> {
    let app_state = live_state(&state)?;
    let (enabled, min_confidence) = {
        let cfg = app_state.config.read().unwrap();
        (cfg.enrichment_enabled, cfg.enrichment_min_confidence)
    };
    let coverage = crate::api::coverage(&app_state.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(EnrichmentStatus {
        enabled,
        min_confidence,
        coverage,
        job: crate::api::latest_enrich_job(&app_state),
    })
}

/// Turn online lookup on or off and set its threshold: applied live (off
/// cancels a lookup in progress, on queues one) and saved to the config file.
#[tauri::command]
pub async fn setup_set_enrichment(
    enabled: bool,
    min_confidence: f32,
    state: tauri::State<'_, DesktopState>,
) -> Result<(), String> {
    let app_state = live_state(&state)?;
    crate::api::set_enrichment(&app_state, enabled, min_confidence)
        .await
        .map_err(user_msg)?;
    let path = ServerConfig::resolve_path(None).map_err(|e| e.to_string())?;
    let mut on_disk = ServerConfig::load(&path).unwrap_or_default();
    on_disk.enrichment_enabled = enabled;
    on_disk.enrichment_min_confidence = min_confidence;
    on_disk.save(&path).map_err(|e| e.to_string())
}

/// Start, pause, resume or cancel album info lookup. `job_id` is needed for
/// all but start.
#[tauri::command]
pub async fn setup_enrichment_action(
    action: String,
    job_id: Option<String>,
    state: tauri::State<'_, DesktopState>,
) -> Result<(), String> {
    let s = live_state(&state)?;
    let id = || job_id.clone().ok_or_else(|| "no job".to_string());
    let result = match action.as_str() {
        "start" => crate::api::start_enrich_job(&s, "Album info lookup")
            .await
            .map(drop),
        "pause" => crate::api::pause_enrich(&s, &id()?).await.map(drop),
        "resume" => crate::api::resume_enrich(&s, &id()?).await.map(drop),
        "cancel" => crate::api::cancel_enrich(&s, &id()?).await.map(drop),
        other => return Err(format!("unknown action '{other}'")),
    };
    result.map_err(user_msg)
}

/// Spawns `run_server`, then waits briefly so an immediate bind failure has
/// time to surface — `run_server` errors out fast on a bad bind address,
/// well before the (long-running) `axum::serve` call.
///
/// Uses `tauri::async_runtime::spawn`, not `tokio::spawn` directly: this can
/// run from `.setup()`, which fires on the main thread before Tauri's event
/// loop (and its Tokio runtime) is entered — a bare `tokio::spawn` there
/// panics with "no reactor running". Tauri's wrapper owns/lazily-inits its
/// own runtime instead of relying on the calling thread already being in one.
async fn spawn_and_check(
    state: DesktopState,
    config: ServerConfig,
) -> Result<ServerStatus, String> {
    let bind = config.bind.clone();
    let last_error = state.last_error.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::run_server_with_ready(config, Some(ready_tx)).await {
            *last_error.lock().unwrap() = Some(e.to_string());
        }
    });
    // Capture the live AppState as soon as it exists (before bind/serve),
    // so it's available even if the bind itself then fails below.
    if let Ok(app_state) = ready_rx.await {
        *state.app_state.lock().unwrap() = Some(app_state);
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let finished = handle.inner().is_finished();
    *state.server_task.lock().unwrap() = Some(handle);
    if finished {
        let msg = state
            .last_error
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "the server exited immediately after starting".to_string());
        Err(msg)
    } else {
        *state.bind.lock().unwrap() = Some(bind.clone());
        Ok(ServerStatus {
            running: true,
            bind,
        })
    }
}

#[tauri::command]
pub async fn setup_start_server(
    state: tauri::State<'_, DesktopState>,
) -> Result<ServerStatus, String> {
    let path = ServerConfig::resolve_path(None).map_err(|e| e.to_string())?;
    let config = ServerConfig::load(&path).map_err(|e| e.to_string())?;
    spawn_and_check(state.inner().clone(), config).await
}

/// Stops the server process (not the desktop app) without quitting: aborts
/// the running task and drops the `AppState` handle. `setup_start_server` /
/// `setup_restart_server` can bring it back up without relaunching the app.
#[tauri::command]
pub fn setup_stop_server(state: tauri::State<'_, DesktopState>) {
    if let Some(handle) = state.server_task.lock().unwrap().take() {
        handle.inner().abort();
    }
    *state.app_state.lock().unwrap() = None;
    *state.bind.lock().unwrap() = None;
}

/// Stop, then start again from the on-disk config — e.g. after an Advanced
/// settings change (bind/database) that can't be hot-applied.
#[tauri::command]
pub async fn setup_restart_server(
    state: tauri::State<'_, DesktopState>,
) -> Result<ServerStatus, String> {
    setup_stop_server(state.clone());
    setup_start_server(state).await
}

#[tauri::command]
pub fn setup_server_status(state: tauri::State<'_, DesktopState>) -> ServerStatus {
    let running = state
        .server_task
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| !h.inner().is_finished())
        .unwrap_or(false);
    let bind = state.bind.lock().unwrap().clone().unwrap_or_default();
    ServerStatus { running, bind }
}

#[tauri::command]
pub fn setup_reveal_config() {
    if let Ok(path) = ServerConfig::resolve_path(None) {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or(path);
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
}

#[tauri::command]
pub fn setup_quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// On launch: if a config already resolves and loads, start the server in
/// the background right away (the frontend's own `setup_get_state` /
/// `setup_server_status` calls then show the status view). Otherwise leave
/// it be — the frontend shows the wizard, and `setup_start_server` runs
/// this same path once the user finishes it.
pub fn autostart(app: &tauri::App) {
    let state: DesktopState = app.state::<DesktopState>().inner().clone();
    if let Ok(path) = ServerConfig::resolve_path(None) {
        if let Ok(config) = ServerConfig::load(&path) {
            // `.setup()` runs before Tauri's event loop (and its Tokio
            // runtime) starts — see the note on `spawn_and_check`.
            tauri::async_runtime::spawn(async move {
                let _ = spawn_and_check(state, config).await;
            });
        }
    }
}

// --- Splash window ------------------------------------------------------------

/// Shown at least this long, so it never just flashes.
const SPLASH_MIN: std::time::Duration = std::time::Duration::from_millis(1200);
/// Closed after this long even if the UI never says it's ready.
const SPLASH_MAX: std::time::Duration = std::time::Duration::from_secs(20);

pub struct SplashState {
    shown_at: std::time::Instant,
    done: std::sync::atomic::AtomicBool,
}

/// Percent-encode a query value.
fn query_value(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// At launch: the splash window (this build's name and version on the
/// artwork), then the main window, hidden until the UI says it's ready
/// (`setup_app_ready`) or SPLASH_MAX passes.
pub fn open_windows(app: &tauri::App) -> tauri::Result<()> {
    let info = app.package_info();
    let url = format!(
        "splash.html?name={}&version={}",
        query_value(&info.name),
        query_value(&info.version.to_string())
    );
    tauri::WebviewWindowBuilder::new(app, "splash", tauri::WebviewUrl::App(url.into()))
        .title(&info.name)
        .inner_size(720.0, 480.0)
        .resizable(false)
        .decorations(false)
        .center()
        .skip_taskbar(true)
        .build()?;
    app.manage(SplashState {
        shown_at: std::time::Instant::now(),
        done: std::sync::atomic::AtomicBool::new(false),
    });
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .expect("tauri.conf.json has a \"main\" window");
    tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?
        .visible(false)
        .build()?;
    let handle = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(SPLASH_MAX);
        finish_splash(&handle);
    });
    Ok(())
}

/// Show the main window and close the splash, once: after SPLASH_MIN.
fn finish_splash(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<SplashState>() else {
        return;
    };
    if state.done.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let wait = SPLASH_MIN.saturating_sub(state.shown_at.elapsed());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(wait);
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(main) = app2.get_webview_window("main") {
                let _ = main.show();
                let _ = main.set_focus();
            }
            if let Some(splash) = app2.get_webview_window("splash") {
                let _ = splash.close();
            }
        });
    });
}

/// The UI is up (the wizard, or the status view with the server running):
/// swap the splash for the main window.
#[tauri::command]
pub fn setup_app_ready(app: tauri::AppHandle) {
    finish_splash(&app);
}
