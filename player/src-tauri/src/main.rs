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
    device_capabilities, device_live_state, resolved_output_device_name, CpalSink, SinkRouter,
};
use kahawai_player_core::{
    fetch_artwork, is_known_dsd_device, validate_bands, AnalogSettings, ArtworkCache, BitPerfect, DsdStory, DspSettings,
    EngineController, EqBand, OutputPath, PlayerEvent, PlayerSnapshot, PlayerStatus, QualityMode,
    RepeatMode,
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
    /// Where the chosen cap (`ArtworkCacheConfig`) is persisted.
    config_path: PathBuf,
}

/// Default upper bound for the on-disk cover cache (least recently used go
/// first), used the first time the app runs. User-configurable afterwards
/// (Settings → Album art cache) among `ARTWORK_CACHE_SIZE_OPTIONS`.
const ARTWORK_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Choices offered in the Settings dropdown.
const ARTWORK_CACHE_SIZE_OPTIONS: [u64; 3] =
    [512 * 1024 * 1024, 1024 * 1024 * 1024, 2 * 1024 * 1024 * 1024];

/// The one setting this cache needs persisted, kept in its own small file
/// rather than `engine-settings.json` — the cache is a shell-only concern,
/// with nothing for the playback engine to know about.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ArtworkCacheConfig {
    #[serde(default = "default_artwork_cache_bytes")]
    max_bytes: u64,
}

fn default_artwork_cache_bytes() -> u64 {
    ARTWORK_CACHE_BYTES
}

impl ArtworkCacheConfig {
    fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| Self { max_bytes: default_artwork_cache_bytes() })
    }

    fn save(&self, path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, t);
        }
    }
}

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
    max_bytes: u64,
    /// Options for the Settings dropdown.
    size_options: Vec<u64>,
    /// Free space on the volume holding the cache dir; `None` if it
    /// couldn't be read (e.g. an exotic filesystem).
    free_bytes: Option<u64>,
}

fn artwork_cache_stats_for(cache: &ArtworkCache) -> ArtworkCacheStats {
    let (bytes, files) = cache.stats();
    ArtworkCacheStats {
        bytes,
        files,
        max_bytes: cache.max_bytes(),
        size_options: ARTWORK_CACHE_SIZE_OPTIONS.to_vec(),
        free_bytes: fs4::available_space(cache.dir()).ok(),
    }
}

#[tauri::command]
fn artwork_cache_stats(state: State<'_, ArtworkState>) -> ArtworkCacheStats {
    artwork_cache_stats_for(&state.cache)
}

#[tauri::command]
fn clear_artwork_cache(state: State<'_, ArtworkState>) -> usize {
    state.cache.clear()
}

/// Settings → Album art cache: change the cap. Evicts immediately if the
/// cache is already over the new limit.
#[tauri::command]
fn set_artwork_cache_max_bytes(state: State<'_, ArtworkState>, max_bytes: u64) -> ArtworkCacheStats {
    state.cache.set_max_bytes(max_bytes);
    ArtworkCacheConfig { max_bytes }.save(&state.config_path);
    artwork_cache_stats_for(&state.cache)
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
    /// Look-ahead limiter gain reduction, dB. None while off or bypassed.
    limiter_gr_db: Option<f32>,
    format: Option<&'static str>,
    chain: Option<String>,
    /// "pcm-shared" | "dop-exclusive" — drives the Exclusive DoP badge.
    output_path: &'static str,
    volume: f32,
    error: Option<String>,
    /// Non-fatal explanation for this track (e.g. DSD played as FLAC, and why).
    notice: Option<String>,
    /// User processing (EQ, Loudness, Analog, Volume) exclusive output would bypass right now.
    exclusive_blockers: Vec<String>,
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
            limiter_gr_db: s.limiter_gr_db,
            format: s.format.map(format_str),
            chain: s.chain,
            output_path: output_path_str(s.output_path),
            volume: s.volume,
            error: s.error,
            notice: s.notice,
            exclusive_blockers: s.exclusive_blockers,
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

/// Move a queue entry (list positions) without interrupting playback. No
/// `emit_state`: the playback thread emits the new queue itself once applied.
#[tauri::command]
fn queue_move(state: State<'_, AppState>, from: usize, to: usize) {
    state.engine.move_queue_item(from, to);
}

/// Remove a queue entry (list position). Playback carries on unless it was the
/// playing track. Same emit rule as `queue_move`.
#[tauri::command]
fn queue_remove(state: State<'_, AppState>, index: usize) {
    state.engine.remove_queue_item(index);
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
    /// "auto" | "native" | "convert".
    dsd_story: &'static str,
    global_format: Option<&'static str>,
    /// "auto" | "off" | "mqa" | "all" — exclusive bit-perfect output.
    bit_perfect: &'static str,
    /// "best" | "compatible".
    quality_mode: &'static str,
}

fn quality_str(m: QualityMode) -> &'static str {
    match m {
        QualityMode::Best => "best",
        QualityMode::Compatible => "compatible",
    }
}

fn bit_perfect_str(m: BitPerfect) -> &'static str {
    match m {
        BitPerfect::Auto => "auto",
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
            DsdStory::Auto => "auto",
        },
        global_format: fmt.map(format_str),
        bit_perfect: bit_perfect_str(state.engine.bit_perfect()),
        quality_mode: quality_str(state.engine.quality_mode()),
    }
}

/// DSD handling preference: "auto" (native on known DoP DACs, else convert),
/// "native" (request DoP when nothing overrides) or "convert" (DSD → PCM). Persisted to the engine
/// settings file.
/// Choose when to play through the exclusive bit-perfect path
/// ("off" | "mqa" | "all"). Persisted; a playing track moves over at its
/// current position. No `emit_state` (see `seek_ms`): the playback thread
/// emits the resulting state, including the new output path.
#[tauri::command]
fn set_bit_perfect(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    let m = match mode.as_str() {
        "auto" => BitPerfect::Auto,
        "off" => BitPerfect::Off,
        "mqa" => BitPerfect::Mqa,
        "all" => BitPerfect::All,
        _ => return Err(format!("unknown bit-perfect mode: {mode}")),
    };
    state.engine.set_bit_perfect(m);
    Ok(())
}

/// Top-level quality mode: "best" | "compatible". Persisted; a playing track
/// re-opens under it at the same position.
#[tauri::command]
fn set_quality_mode(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    let m = match mode.as_str() {
        "best" => QualityMode::Best,
        "compatible" => QualityMode::Compatible,
        _ => return Err(format!("unknown quality mode: {mode}")),
    };
    state.engine.set_quality_mode(m);
    Ok(())
}

#[tauri::command]
fn set_dsd_story(app: AppHandle, state: State<'_, AppState>, story: String) -> Result<(), String> {
    let s = match story.as_str() {
        "native" => DsdStory::Native,
        "convert" => DsdStory::Convert,
        "auto" => DsdStory::Auto,
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

#[tauri::command]
fn set_limiter_enabled(app: AppHandle, state: State<'_, AppState>, enabled: bool) {
    state.engine.set_limiter_enabled(enabled);
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
struct DsdRateDto {
    /// "DSD64" | "DSD128" | "DSD256".
    name: &'static str,
    /// DoP PCM rate this DSD rate needs (176400 / 352800 / 705600).
    dop_rate: u32,
    supported: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
struct DopStatusDto {
    /// DoP PCM rates the output device accepts right now
    /// (176400 / 352800 / 705600). Empty on non-macOS.
    supported_rates: Vec<u32>,
    /// The same, named by the DSD rate they carry (for the Settings probe).
    dsd_rates: Vec<DsdRateDto>,
    /// True on macOS: the exclusive hog-mode path exists.
    exclusive_available: bool,
    /// The device output would use (the chosen one, or the system default's
    /// real name); `None` when it can't be resolved.
    device: Option<String>,
    /// Built in, or confirmed by the user, as decoding DoP.
    known_dsd_device: bool,
    /// The user's own confirmation (Settings toggle) is what makes it known.
    user_confirmed: bool,
    /// What "Auto" resolves to right now: "native" | "convert".
    auto_resolves_to: &'static str,
    /// Everything the device reports it can carry (Settings shows it at a glance).
    capabilities: Option<DeviceCapsDto>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct DeviceCapsDto {
    name: String,
    /// "usb" | "thunderbolt" | "firewire" | "built-in" | "bluetooth" | "hdmi" | ...
    transport: &'static str,
    /// External DAC-class connection: Best quality may take it exclusively.
    external_dac: bool,
    sample_rates: Vec<u32>,
    bit_depths: Vec<u32>,
    float32: bool,
    dop_rates: Vec<u32>,
    exclusive_available: bool,
}

#[tauri::command]
fn dop_status(state: State<'_, AppState>) -> DopStatusDto {
    let chosen = state.engine.output_device();
    let rates = dop_capable_rates(chosen.as_deref());
    let device = resolved_output_device_name(chosen.as_deref());
    let confirmed = state.engine.dsd_devices();
    let known = device
        .as_deref()
        .map(|d| is_known_dsd_device(d, &confirmed))
        .unwrap_or(false);
    let user_confirmed = device
        .as_deref()
        .map(|d| confirmed.iter().any(|c| c.trim().eq_ignore_ascii_case(d.trim())))
        .unwrap_or(false);
    let dsd_rates = [("DSD64", 176_400u32), ("DSD128", 352_800), ("DSD256", 705_600)]
        .into_iter()
        .map(|(name, dop_rate)| DsdRateDto {
            name,
            dop_rate,
            supported: rates.contains(&dop_rate),
        })
        .collect();
    DopStatusDto {
        exclusive_available: cfg!(target_os = "macos"),
        supported_rates: rates,
        dsd_rates,
        device,
        known_dsd_device: known,
        user_confirmed,
        auto_resolves_to: if known { "native" } else { "convert" },
        capabilities: device_capabilities(chosen.as_deref()).map(|c| DeviceCapsDto {
            name: c.name,
            transport: c.transport,
            external_dac: c.external_dac,
            sample_rates: c.sample_rates,
            bit_depths: c.bit_depths,
            float32: c.float32,
            dop_rates: c.dop_rates,
            exclusive_available: c.exclusive_available,
        }),
    }
}

/// What the output device is doing right now (its real rate and stream
/// format, read from the OS), for the live signal-path panel in Settings.
#[derive(Debug, Clone, serde::Serialize)]
struct OutputLiveDto {
    name: String,
    rate_hz: u32,
    /// 0 when unknown.
    bit_depth: u32,
    /// The stream format is floating point (the shared mixer's).
    float: bool,
    /// This app holds the device exclusively.
    exclusive: bool,
}

#[tauri::command]
fn output_live_state(state: State<'_, AppState>) -> Option<OutputLiveDto> {
    let chosen = state.engine.output_device();
    device_live_state(chosen.as_deref()).map(|l| OutputLiveDto {
        name: l.name,
        rate_hz: l.rate_hz,
        bit_depth: l.bit_depth,
        float: l.float,
        exclusive: l.exclusive,
    })
}

/// Mark the current output device as (not) decoding DoP, so "Auto" DSD
/// handling goes native for it. Persisted.
#[tauri::command]
fn set_dsd_device_confirmed(state: State<'_, AppState>, confirmed: bool) -> Result<(), String> {
    let chosen = state.engine.output_device();
    let name = resolved_output_device_name(chosen.as_deref())
        .ok_or_else(|| "output device unknown".to_string())?;
    state.engine.set_dsd_device_confirmed(&name, confirmed);
    Ok(())
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
            // The cap itself is a shell-only setting, kept in its own small
            // file next to engine-settings.json.
            let art_dir = app
                .path()
                .app_cache_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("kahawai-player"))
                .join("artwork");
            let artwork_config_path = app
                .path()
                .app_config_dir()
                .map(|d| d.join("artwork-cache.json"))
                .unwrap_or_else(|_| std::env::temp_dir().join("kahawai-player-artwork-cache.json"));
            let max_bytes = ArtworkCacheConfig::load(&artwork_config_path).max_bytes;
            let cache = ArtworkCache::new(art_dir, max_bytes).map_err(|e| format!("artwork cache: {e}"))?;
            app.manage(ArtworkState {
                cache,
                engine: engine.clone(),
                config_path: artwork_config_path,
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
            set_artwork_cache_max_bytes,
            set_server_url,
            play_track,
            queue_play,
            queue_move,
            queue_remove,
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
            set_limiter_enabled,
            get_dsp_settings,
            dop_status,
            set_dsd_device_confirmed,
            set_quality_mode,
            output_live_state,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Quitting: let the engine save the live playhead now. The
            // periodic save only runs every 5 s, and Tauri does not drop
            // managed state on exit, so the engine's Drop never runs.
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.engine.shutdown();
                }
            }
        });
}
