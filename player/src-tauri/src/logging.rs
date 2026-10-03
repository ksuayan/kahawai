//! Conditional tracing: dev builds log verbosely, production builds stay
//! quiet, and a Settings toggle ("Verbose logging") overrides at runtime.
//!
//! - Desktop dev: terminal (the existing fmt subscriber).
//! - Android dev: logcat, via `android_logger` plus a tracing layer below.
//! - Production: today's behavior (macOS log file; `info` level).
//! - Every Android build: panics go to logcat. A silent native crash helps
//!   nobody, and logcat is only readable over adb.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tracing_subscriber::{filter::EnvFilter, layer::SubscriberExt, reload};

/// Handle for swapping the log filter at runtime (the Verbose logging
/// toggle). Set once in [`init`].
static FILTER_HANDLE: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> =
    OnceLock::new();

/// Shell-only setting, kept in its own small file next to
/// `engine-settings.json` (the same pattern as `ArtworkCacheConfig`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct LoggingConfig {
    /// Debug-level tracing in a production build. Dev builds are always
    /// verbose; the toggle only adds verbosity to prod.
    #[serde(default)]
    pub verbose: bool,
}

impl LoggingConfig {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    pub fn path_for(config_dir: &Path) -> PathBuf {
        config_dir.join("logging.json")
    }
}

/// Effective level: dev builds are always verbose; prod follows the toggle.
/// `RUST_LOG` still wins when set (handled in [`init`]).
fn level(verbose: bool) -> &'static str {
    if cfg!(debug_assertions) || verbose {
        "debug"
    } else {
        "info"
    }
}

/// Install the process-wide tracing subscriber. The reload handle is stashed
/// for [`apply_verbose`].
pub fn init() {
    let (filter, handle) = reload::Layer::new(
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level(false))),
    );
    let subscriber = tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout())),
    );
    #[cfg(target_os = "android")]
    let subscriber = subscriber.with(LogcatLayer);
    tracing::subscriber::set_global_default(subscriber).expect("tracing subscriber already set");
    let _ = FILTER_HANDLE.set(handle);

    #[cfg(target_os = "android")]
    {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Trace)
                .with_tag("kahawai"),
        );
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}");
            prev(info);
        }));
    }
}

/// Apply the persisted Verbose logging toggle (called from setup, once the
/// config dir is known).
pub fn apply_verbose(verbose: bool) {
    if let Some(handle) = FILTER_HANDLE.get() {
        let _ = handle.modify(|f| *f = EnvFilter::new(level(verbose)));
    }
}

/// Tracing → logcat layer (Android only). `android_logger` owns the logcat
/// FFI; this forwards tracing events into the `log` facade.
#[cfg(target_os = "android")]
struct LogcatLayer;

#[cfg(target_os = "android")]
struct LogcatFields(String);

#[cfg(target_os = "android")]
impl tracing::field::Visit for LogcatFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

#[cfg(target_os = "android")]
impl<S> tracing_subscriber::Layer<S> for LogcatLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = LogcatFields(String::new());
        event.record(&mut fields);
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => log::Level::Error,
            tracing::Level::WARN => log::Level::Warn,
            tracing::Level::INFO => log::Level::Info,
            tracing::Level::DEBUG => log::Level::Debug,
            tracing::Level::TRACE => log::Level::Trace,
        };
        log::log!(level, "[{}] {}", event.metadata().target(), fields.0);
    }
}
