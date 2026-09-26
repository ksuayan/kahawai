//! Kahawai Player — Tauri 2 shell (C2).
//!
//! The shell is deliberately thin: it owns an [`EngineController`] on its
//! dedicated playback thread, forwards the engine's state events to the
//! Vue UI as `player-state`, and resolves `play_track` ids through
//! [`kahawai_player_api`]. All playback logic lives in `kahawai-player-core`; audio output
//! is a [`SinkRouter`] of [`CpalSink`] (shared-mode PCM) and the platform
//! exclusive DoP sink (macOS hog-mode CoreAudio; a no-op stub elsewhere).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod menu;

use kahawai_core::{StreamFormat, Track};
use kahawai_player_api::Client as ApiClient;
use kahawai_player_audio::{
    dop_capable_rates, exclusive_dop_sink, list_output_devices as audio_list_output_devices,
    CpalSink, SinkRouter,
};
use kahawai_player_core::{
    fetch_artwork, validate_bands, AnalogSettings, ArtworkCache, BitPerfect, DsdStory, DspSettings,
    EngineController, EqBand, OutputPath, PlayerEvent, PlayerSnapshot, PlayerStatus, RepeatMode,
};
use tauri::{AppHandle, Emitter, Manager, State};

/// Shared shell state. The engine is `Arc` so the event-forwarding thread
/// can drain events while commands borrow it.
struct AppState {
    engine: Arc<EngineController>,
    api: Mutex<ApiClient>,
    settings_path: PathBuf,
}

/// Album-art disk cache + the engine (for the live server URL). Served to
/// the webview through the `artwork://` protocol; see `serve_artwork`.
struct ArtworkState {
    cache: ArtworkCache,
    engine: Arc<EngineController>,
}

/// Upper bound for the on-disk cover cache (least recently used go first).
const ARTWORK_CACHE_BYTES: u64 = 512 * 1024 * 1024;

/// `artwork://localhost/<hash>` → cached bytes, fetching from the server on
/// a miss. Runs on a worker thread: the request handler must not block the
/// UI thread on the network.
fn serve_artwork(
    app: AppHandle,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let hash = request.uri().path().trim_start_matches('/').to_string();
    std::thread::spawn(move || {
        let reply = |status: u16, body: Vec<u8>, mime: &str, cache: &str| {
            tauri::http::Response::builder()
                .status(status)
                .header("Content-Type", mime)
                .header("Cache-Control", cache)
                .header("Access-Control-Allow-Origin", "*")
                .body(body)
                .expect("static response headers are valid")
        };
        let Some(st) = app.try_state::<ArtworkState>() else {
            return responder.respond(reply(503, Vec::new(), "text/plain", "no-store"));
        };
        let server = st.engine.server_url();
        match st
            .cache
            .get_or_fetch(&hash, || fetch_artwork(&server, &hash))
        {
            // Content-addressed: the webview may keep it forever too.
            Ok(art) => responder.respond(reply(
                200,
                art.bytes,
                art.mime,
                "public, max-age=31536000, immutable",
            )),
            Err(e) => {
                let status = match e {
                    kahawai_core::MusicError::BadRequest(_) => 400,
                    kahawai_core::MusicError::NotFound(_) => 404,
                    _ => 502,
                };
                // Failures must not be cached by the webview: the server
                // may just be starting.
                responder.respond(reply(status, Vec::new(), "text/plain", "no-store"));
            }
        }
    });
}

#[derive(Debug, Clone, serde::Serialize)]
struct ArtworkCacheStats {
    bytes: u64,
    files: usize,
}

#[tauri::command]
fn artwork_cache_stats(state: State<'_, ArtworkState>) -> ArtworkCacheStats {
    let (bytes, files) = state.cache.stats();
    ArtworkCacheStats { bytes, files }
}

#[tauri::command]
fn clear_artwork_cache(state: State<'_, ArtworkState>) -> usize {
    state.cache.clear()
}

/// The analog stage's effect on the level (matches `AnalogLevel` in the UI types).
#[derive(Debug, Clone, serde::Serialize)]
struct AnalogLevelDto {
    input_lufs: f32,
    output_lufs: f32,
    delta_db: f32,
    peak_dbfs: f32,
    seconds: f32,
}

/// `player-state` payload. Field names match `ui/src/types.ts PlayerState`
/// exactly (snake_case).
#[derive(Debug, Clone, serde::Serialize)]
struct PlayerStateDto {
    status: &'static str,
    track: Option<Track>,
    queue_ids: Vec<i64>,
    queue_index: Option<usize>,
    position_ms: u64,
    duration_ms: Option<u64>,
    /// How far the data received from the server reaches into the track.
    buffered_ms: Option<u64>,
    /// Rate of the audio reaching the output; the rate the EQ is designed at.
    output_rate_hz: Option<u32>,
    /// What the analog stage is doing right now (plan and latency), if on.
    analog_plan: Option<String>,
    /// How the analog stage changes the level (before/after loudness, peak).
    analog_level: Option<AnalogLevelDto>,
    format: Option<&'static str>,
    chain: Option<String>,
    /// "pcm-shared" | "dop-exclusive" — drives the Exclusive DoP badge.
    output_path: &'static str,
    volume: f32,
    error: Option<String>,
    /// "off" | "all" | "one".
    repeat: &'static str,
    shuffle: bool,
}

fn status_str(s: PlayerStatus) -> &'static str {
    match s {
        PlayerStatus::Stopped => "stopped",
        PlayerStatus::Loading => "loading",
        PlayerStatus::Playing => "playing",
        PlayerStatus::Paused => "paused",
    }
}

fn format_str(f: StreamFormat) -> &'static str {
    match f {
        StreamFormat::Passthrough => "passthrough",
        StreamFormat::Flac => "flac",
        StreamFormat::Opus => "opus",
        StreamFormat::Mp3 => "mp3",
        StreamFormat::Dop => "dop",
    }
}

fn output_path_str(p: OutputPath) -> &'static str {
    match p {
        OutputPath::Pcm => "pcm-shared",
        OutputPath::Dop => "dop-exclusive",
        OutputPath::PcmExclusive => "pcm-exclusive",
    }
}

fn repeat_str(m: RepeatMode) -> &'static str {
    match m {
        RepeatMode::Off => "off",
        RepeatMode::All => "all",
        RepeatMode::One => "one",
    }
}

impl From<PlayerSnapshot> for PlayerStateDto {
    fn from(s: PlayerSnapshot) -> Self {
        Self {
            status: status_str(s.status),
            track: s.track,
            queue_ids: s.queue_ids,
            queue_index: s.queue_index,
            position_ms: s.position_ms,
            duration_ms: s.duration_ms,
            buffered_ms: s.buffered_ms,
            output_rate_hz: s.output_rate_hz,
            analog_plan: s.analog_plan,
            analog_level: s.analog_level.map(|l| AnalogLevelDto {
                input_lufs: l.input_lufs,
                output_lufs: l.output_lufs,
                delta_db: l.delta_db,
                peak_dbfs: l.peak_dbfs,
                seconds: l.seconds,
            }),
            format: s.format.map(format_str),
            chain: s.chain,
            output_path: output_path_str(s.output_path),
            volume: s.volume,
            error: s.error,
            repeat: repeat_str(s.repeat),
            shuffle: s.shuffle,
        }
    }
}

fn emit_state(app: &AppHandle, engine: &EngineController) {
    let dto = PlayerStateDto::from(engine.snapshot());
    if let Err(e) = app.emit("player-state", dto) {
        eprintln!("[shell] player-state emit failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_state(state: State<'_, AppState>) -> PlayerStateDto {
    PlayerStateDto::from(state.engine.snapshot())
}

/// The tracks of the saved queue (from `queue.json`), so the UI can show a
/// restored queue without asking the server for each one.
#[tauri::command]
fn get_queue_tracks(state: State<'_, AppState>) -> Vec<Track> {
    state.engine.saved_queue_tracks()
}

#[tauri::command]
fn get_server_url(state: State<'_, AppState>) -> String {
    state.engine.server_url()
}

#[tauri::command]
fn set_server_url(app: AppHandle, state: State<'_, AppState>, url: String) -> Result<(), String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return Err("server URL must not be empty".to_string());
    }
    *state
        .api
        .lock()
        .map_err(|_| "api lock poisoned".to_string())? = ApiClient::new(url.clone());
    state.engine.set_server_url(&url);
    emit_state(&app, &state.engine);
    Ok(())
}

/// Play one track by catalog id. Metadata is resolved through the browse
/// API; the track becomes a one-item queue.
#[tauri::command]
async fn play_track(app: AppHandle, state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let api: ApiClient = state
        .api
        .lock()
        .map_err(|_| "api lock poisoned".to_string())?
        .clone();
    let track = api.track(id).await.map_err(|e| e.to_string())?;
    state.engine.play_queue(vec![track], 0);
    emit_state(&app, &state.engine);
    Ok(())
}

/// Replace the queue with full `Track` objects (the UI owns the list) and
/// start at `index`.
#[tauri::command]
fn queue_play(
    app: AppHandle,
    state: State<'_, AppState>,
    tracks: Vec<Track>,
    index: usize,
) -> Result<(), String> {
    state.engine.play_queue(tracks, index);
    emit_state(&app, &state.engine);
    Ok(())
}

#[tauri::command]
fn pause(app: AppHandle, state: State<'_, AppState>) {
    state.engine.pause();
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn resume(app: AppHandle, state: State<'_, AppState>) {
    state.engine.resume();
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn toggle(app: AppHandle, state: State<'_, AppState>) {
    state.engine.toggle();
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn stop(app: AppHandle, state: State<'_, AppState>) {
    state.engine.stop();
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn seek_ms(state: State<'_, AppState>, ms: u64) {
    // No `emit_state` here: the snapshot is refreshed by the playback
    // thread *after* it applies the command, so emitting now would send the
    // pre-seek position. The thread emits the real state itself.
    state.engine.seek_ms(ms);
}

#[tauri::command]
fn next(app: AppHandle, state: State<'_, AppState>) {
    state.engine.next();
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn prev(app: AppHandle, state: State<'_, AppState>) {
    state.engine.prev();
    emit_state(&app, &state.engine);
}

/// Frontend aliases: the Vue bridge invokes `next_track` / `prev_track`.
#[tauri::command]
fn next_track(app: AppHandle, state: State<'_, AppState>) {
    next(app, state);
}

#[tauri::command]
fn prev_track(app: AppHandle, state: State<'_, AppState>) {
    prev(app, state);
}

/// Global format override; `None` clears it (ladder decides per track).
#[tauri::command]
fn set_format(app: AppHandle, state: State<'_, AppState>, fmt: Option<StreamFormat>) {
    state.engine.set_global_format(fmt);
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn set_track_format(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: i64,
    fmt: Option<StreamFormat>,
) {
    state.engine.set_track_format(track_id, fmt);
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn set_volume(state: State<'_, AppState>, v: f32) {
    // See `seek_ms`: the playback thread emits the updated snapshot; an
    // immediate emit here carried the OLD volume and snapped the slider back.
    state.engine.set_volume(v.clamp(0.0, 1.0));
}

// ---------------------------------------------------------------------------
// C3: queue actions, repeat/shuffle, DSD story
// ---------------------------------------------------------------------------

#[tauri::command]
fn set_repeat(app: AppHandle, state: State<'_, AppState>, mode: String) -> Result<(), String> {
    let m = match mode.as_str() {
        "off" => RepeatMode::Off,
        "all" => RepeatMode::All,
        "one" => RepeatMode::One,
        _ => return Err(format!("unknown repeat mode: {mode}")),
    };
    state.engine.set_repeat(m);
    emit_state(&app, &state.engine);
    Ok(())
}

#[tauri::command]
fn set_shuffle(app: AppHandle, state: State<'_, AppState>, on: bool) {
    state.engine.set_shuffle(on);
    emit_state(&app, &state.engine);
}

/// Append tracks to the end of the queue ("add to queue"); current
/// playback is undisturbed.
#[tauri::command]
fn queue_append(app: AppHandle, state: State<'_, AppState>, tracks: Vec<Track>) {
    state.engine.append_tracks(tracks);
    emit_state(&app, &state.engine);
}

/// Insert tracks right after the current queue item ("play next").
#[tauri::command]
fn queue_insert_next(app: AppHandle, state: State<'_, AppState>, tracks: Vec<Track>) {
    state.engine.insert_tracks_next(tracks);
    emit_state(&app, &state.engine);
}

#[derive(Debug, Clone, serde::Serialize)]
struct PlaybackPrefsDto {
    /// "native" | "convert".
    dsd_story: &'static str,
    global_format: Option<&'static str>,
    /// "off" | "mqa" | "all" — exclusive bit-perfect output.
    bit_perfect: &'static str,
}

fn bit_perfect_str(m: BitPerfect) -> &'static str {
    match m {
        BitPerfect::Off => "off",
        BitPerfect::Mqa => "mqa",
        BitPerfect::All => "all",
    }
}

/// Persisted playback preferences (DSD story + global format override).
/// The UI calls this once at startup; repeat/shuffle ride in `get_state`.
#[tauri::command]
fn get_playback_prefs(state: State<'_, AppState>) -> PlaybackPrefsDto {
    let (story, fmt) = state.engine.playback_prefs();
    PlaybackPrefsDto {
        dsd_story: match story {
            DsdStory::Native => "native",
            DsdStory::Convert => "convert",
        },
        global_format: fmt.map(format_str),
        bit_perfect: bit_perfect_str(state.engine.bit_perfect()),
    }
}

/// DSD handling preference: "native" (request DoP when nothing overrides)
/// or "convert" (DSD → PCM, the pre-C3 default). Persisted to the engine
/// settings file.
/// Choose when to play through the exclusive bit-perfect path
/// ("off" | "mqa" | "all"). Persisted; a playing track moves over at its
/// current position. No `emit_state` (see `seek_ms`): the playback thread
/// emits the resulting state, including the new output path.
#[tauri::command]
fn set_bit_perfect(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    let m = match mode.as_str() {
        "off" => BitPerfect::Off,
        "mqa" => BitPerfect::Mqa,
        "all" => BitPerfect::All,
        _ => return Err(format!("unknown bit-perfect mode: {mode}")),
    };
    state.engine.set_bit_perfect(m);
    Ok(())
}

#[tauri::command]
fn set_dsd_story(app: AppHandle, state: State<'_, AppState>, story: String) -> Result<(), String> {
    let s = match story.as_str() {
        "native" => DsdStory::Native,
        "convert" => DsdStory::Convert,
        _ => return Err(format!("unknown DSD story: {story}")),
    };
    state.engine.set_dsd_story(s);
    emit_state(&app, &state.engine);
    Ok(())
}

// ---------------------------------------------------------------------------
// C2: audio devices, DSP (EQ + loudness), DoP capability
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
struct DeviceDto {
    name: String,
    is_default: bool,
}

/// Output devices the user can choose from (names are exact: cpal offers
/// no other identity). Infallible: enumeration failures yield an empty list.
#[tauri::command]
fn get_output_devices() -> Vec<DeviceDto> {
    audio_list_output_devices()
        .into_iter()
        .map(|d| DeviceDto {
            name: d.name,
            is_default: d.is_default,
        })
        .collect()
}

/// The chosen output device (`None` = follow the system default).
#[tauri::command]
fn get_output_device(state: State<'_, AppState>) -> Option<String> {
    state.engine.output_device()
}

/// Choose the output device (`None` = system default). Persisted; a playing
/// track moves over at its current position. No `emit_state` (see
/// `seek_ms`): the playback thread emits the resulting state.
#[tauri::command]
fn set_output_device(state: State<'_, AppState>, name: Option<String>) {
    state.engine.set_output_device(name);
}

/// Replace the parametric EQ bands (≤ 8; validated before anything is
/// sent to the engine). PCM only — DoP bypasses EQ entirely.
#[tauri::command]
fn set_eq_bands(
    app: AppHandle,
    state: State<'_, AppState>,
    bands: Vec<EqBand>,
) -> Result<(), String> {
    validate_bands(&bands).map_err(|e| e.to_string())?;
    state.engine.set_eq_bands(bands);
    emit_state(&app, &state.engine);
    Ok(())
}

/// Analog character (tube / transistor warmth). Values are clamped by the
/// engine, which saves them and applies them live. PCM shared path only.
#[tauri::command]
fn set_analog(app: AppHandle, state: State<'_, AppState>, settings: AnalogSettings) {
    state.engine.set_analog(settings);
    emit_state(&app, &state.engine);
}

#[tauri::command]
fn set_eq_enabled(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    state.engine.set_eq_enabled(enabled);
    emit_state(&app, &state.engine);
}

/// Loudness target in LUFS (default −14). Enabling triggers one extra
/// stream per first-play (pre-scan); gains are cached by (track, format).
/// PCM only — DoP bypasses loudness.
#[tauri::command]
fn set_loudness_target(
    app: AppHandle,
    state: State<'_, AppState>,
    lufs: f32,
) -> Result<(), String> {
    if !lufs.is_finite() || !(-40.0..=-1.0).contains(&lufs) {
        return Err(format!("loudness target out of range: {lufs}"));
    }
    state.engine.set_loudness_target(lufs);
    emit_state(&app, &state.engine);
    Ok(())
}

#[tauri::command]
fn set_loudness_enabled(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    state.engine.set_loudness_enabled(enabled);
    emit_state(&app, &state.engine);
}

/// Persisted DSP settings (the settings file is the source of truth; the
/// engine's setters write it synchronously). The UI calls this once at
/// startup to mirror the engine.
#[tauri::command]
fn get_dsp_settings(state: State<'_, AppState>) -> Result<DspSettings, String> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        dsp: DspSettings,
    }
    let text = std::fs::read_to_string(&state.settings_path).map_err(|e| e.to_string())?;
    let file: File = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(file.dsp)
}

#[derive(Debug, Clone, serde::Serialize)]
struct DopStatusDto {
    /// DoP PCM rates the default output device accepts right now
    /// (176400 / 352800 / 705600). Empty on non-macOS.
    supported_rates: Vec<u32>,
    /// True on macOS: the exclusive hog-mode path exists.
    exclusive_available: bool,
}

#[tauri::command]
fn dop_status(state: State<'_, AppState>) -> DopStatusDto {
    let rates = dop_capable_rates(state.engine.output_device().as_deref());
    DopStatusDto {
        exclusive_available: cfg!(target_os = "macos"),
        supported_rates: rates,
    }
}

// ---------------------------------------------------------------------------
// App setup
// ---------------------------------------------------------------------------

fn main() {
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("artwork", |ctx, request, responder| {
            serve_artwork(ctx.app_handle().clone(), request, responder);
        })
        .on_menu_event(menu::on_menu_event)
        .setup(|app| {
            // macOS menu bar with a custom About item (the UI shows about.md).
            #[cfg(target_os = "macos")]
            app.set_menu(menu::build_app_menu(app.handle())?)?;
            // Persisted engine settings (server URL) live in the app config
            // dir; fall back to a temp file if the dir is unavailable.
            let settings_path = app
                .path()
                .app_config_dir()
                .map(|d| d.join("engine-settings.json"))
                .unwrap_or_else(|_| std::env::temp_dir().join("kahawai-player-settings.json"));
            if let Some(parent) = settings_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            // Real sinks (C2): shared-mode PCM + exclusive DoP router.
            let sink = SinkRouter::new(Box::new(CpalSink::new()), Some(exclusive_dop_sink()));
            let engine = Arc::new(EngineController::new(Box::new(sink), settings_path.clone()));
            let server_url = engine.server_url();
            let api = Mutex::new(ApiClient::new(server_url));

            // Album-art cache: the OS cache dir (safe for the OS to purge).
            let art_dir = app
                .path()
                .app_cache_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("kahawai-player"))
                .join("artwork");
            let cache = ArtworkCache::new(art_dir, ARTWORK_CACHE_BYTES)
                .map_err(|e| format!("artwork cache: {e}"))?;
            app.manage(ArtworkState {
                cache,
                engine: engine.clone(),
            });

            let handle = app.handle();
            let engine2 = engine.clone();
            app.manage(AppState {
                engine,
                api,
                settings_path,
            });

            // Forward engine state events to the UI. The engine already
            // emits at ~4 Hz while playing and immediately on track/state
            // changes; this thread just bridges them into Tauri events.
            let handle2 = handle.clone();
            std::thread::Builder::new()
                .name("player-state-forward".into())
                .spawn(move || loop {
                    for event in engine2.drain_events() {
                        match event {
                            PlayerEvent::State(snap) => {
                                let dto = PlayerStateDto::from(snap);
                                if let Err(e) = handle2.emit("player-state", dto) {
                                    eprintln!("[shell] player-state emit failed: {e}");
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                })
                .expect("spawn player-state-forward thread");

            // Initial snapshot so the UI renders before the first change.
            let state: State<'_, AppState> = app.state();
            emit_state(&handle, &state.engine);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_queue_tracks,
            set_bit_perfect,
            get_server_url,
            artwork_cache_stats,
            clear_artwork_cache,
            set_server_url,
            play_track,
            queue_play,
            pause,
            resume,
            toggle,
            stop,
            seek_ms,
            next,
            prev,
            next_track,
            prev_track,
            set_format,
            set_track_format,
            set_volume,
            set_repeat,
            set_shuffle,
            queue_append,
            queue_insert_next,
            get_playback_prefs,
            set_dsd_story,
            get_output_devices,
            get_output_device,
            set_output_device,
            set_eq_bands,
            set_eq_enabled,
            set_analog,
            set_loudness_target,
            set_loudness_enabled,
            get_dsp_settings,
            dop_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
