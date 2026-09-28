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

#[derive(Serialize)]
pub struct DirValidation {
    exists: bool,
    is_dir: bool,
    readable: bool,
    writable: bool,
    audio_files: usize,
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
        };
    }
    let readable = std::fs::read_dir(dir).is_ok();
    let writable = is_writable(dir);
    let mut audio_files = 0usize;
    if readable {
        let mut seen = 0usize;
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            seen += 1;
            if crate::scanner::is_audio(entry.path()) {
                audio_files += 1;
            }
            if seen >= 2000 {
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
    music_dirs: Vec<String>,
}

/// Add/remove music folders on a *running* server, no restart: updates the
/// live config in place, persists the same change to disk (so a future
/// restart keeps it), then triggers a rescan of the updated directory list.
/// Deliberately scoped to `music_dirs` only — `bind`/`db_path` can't be
/// hot-swapped safely (the listener is already bound; the DB pool is
/// already open against the old path), so those still need the wizard's
/// save-and-restart path.
#[tauri::command]
pub async fn setup_apply_config(
    input: ApplyConfigInput,
    state: tauri::State<'_, DesktopState>,
) -> Result<(), String> {
    if input.music_dirs.is_empty() {
        return Err("Add at least one music folder.".to_string());
    }
    let dirs: Vec<PathBuf> = input.music_dirs.into_iter().map(PathBuf::from).collect();

    let app_state = state
        .app_state
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "the server is not running".to_string())?;

    app_state.config.write().unwrap().music_dirs = dirs.clone();

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
