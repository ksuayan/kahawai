//! Server configuration, loaded from TOML. (Spec §3.6.)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::StreamFormat;
use crate::error::MusicError;

/// Which DSD story is active. (Spec §2: S5a vs S5b — two separate features.)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DsdStory {
    /// DSD → PCM (FIR decimation) → FLAC transcode.
    #[default]
    Pcm,
    /// Native DSD passthrough (DoP) to a DSD-capable DAC.
    Native,
}

fn default_bind() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_db_path() -> PathBuf {
    PathBuf::from("data/music.db")
}

fn default_ladder() -> Vec<StreamFormat> {
    // Desktop default per spec §3.4: passthrough when the client can take the
    // source, otherwise FLAC.
    vec![StreamFormat::Passthrough, StreamFormat::Flac]
}

/// v1 server configuration. v1 is LAN-only: no auth, no TLS (spec §3.6, §8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Directories the scanner walks. Empty = scanning disabled.
    #[serde(default)]
    pub music_dirs: Vec<PathBuf>,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
    /// Audiobook folders, kept apart from the music. They are added to the
    /// catalog's audiobook folders when the server starts (a folder added
    /// through the API stays too), and the desktop app edits this list.
    #[serde(default)]
    pub audiobook_dirs: Vec<PathBuf>,
    /// Preferred format ladder, first satisfiable entry wins
    /// (see [`crate::format::transcode_ladder`]).
    #[serde(default = "default_ladder")]
    pub preferred_ladder: Vec<StreamFormat>,
    #[serde(default)]
    pub dsd_story: DsdStory,
    /// Run the library scanner once at startup. Default off.
    #[serde(default)]
    pub scan_on_startup: bool,
    /// Look up albums without a MusicBrainz ID online (MusicBrainz, Cover
    /// Art Archive). Default off: it sends artist and album names off the
    /// LAN, so it is opt-in.
    #[serde(default)]
    pub enrichment_enabled: bool,
    /// Use the online station directory (radio-browser.info) and podcast
    /// directory (Apple's iTunes Search): the search words you type go to
    /// those services. Default off. Stations and feeds you add by hand, and
    /// everything already saved, work without it.
    #[serde(default)]
    pub online_sources_enabled: bool,
    /// Test seam and escape hatch: use this radio-browser.info compatible
    /// server instead of resolving the public mirrors.
    #[serde(default)]
    pub radio_browser_url: Option<String>,
    /// How sure a lookup must be before an album takes its result, 0.5-1.0.
    /// Below it the album is left unmatched: a wrong ID is worse than none.
    #[serde(default = "default_min_confidence")]
    pub enrichment_min_confidence: f32,
    /// Size cap of the transcode cache in MiB: single-track transcodes are
    /// rendered once to `<data>/transcode-cache/` (next to the database) and
    /// served from there with Content-Length and byte ranges, so players
    /// like VLC can seek them. Oldest renders are evicted past the cap.
    /// 0 turns the cache off (every transcode is streamed live).
    #[serde(default = "default_transcode_cache_mb")]
    pub transcode_cache_mb: u64,
}

fn default_transcode_cache_mb() -> u64 {
    8 * 1024
}

fn default_min_confidence() -> f32 {
    0.9
}

impl ServerConfig {
    /// Load from a TOML file. Missing file / parse error → caller decides
    /// (the server falls back to [`ServerConfig::default`] with a warning).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, MusicError> {
        let text = std::fs::read_to_string(path.as_ref()).map_err(MusicError::Io)?;
        toml::from_str(&text).map_err(|e| MusicError::Config(e.to_string()))
    }

    /// Serialize to TOML and write to `path`, creating parent directories
    /// as needed. Used by the desktop setup wizard (macOS) to persist the
    /// config it built interactively.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), MusicError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(MusicError::Io)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| MusicError::Config(e.to_string()))?;
        std::fs::write(path, text).map_err(MusicError::Io)
    }

    /// The per-OS default config path, used when nothing more specific is
    /// given. Reads `HOME`/`XDG_CONFIG_HOME`/`APPDATA` directly — no new
    /// crates for this.
    pub fn default_config_path() -> Result<PathBuf, MusicError> {
        Self::default_config_path_from_env(|k| std::env::var(k).ok())
    }

    /// Same as [`Self::default_config_path`] but takes an env lookup
    /// function, so tests can exercise every branch without mutating the
    /// process environment.
    fn default_config_path_from_env(
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<PathBuf, MusicError> {
        #[cfg(target_os = "macos")]
        {
            let home =
                env("HOME").ok_or_else(|| MusicError::Config("HOME is not set".to_string()))?;
            Ok(PathBuf::from(home).join("Library/Application Support/Kahawai Server/config.toml"))
        }
        #[cfg(target_os = "windows")]
        {
            let appdata = env("APPDATA")
                .ok_or_else(|| MusicError::Config("APPDATA is not set".to_string()))?;
            Ok(PathBuf::from(appdata)
                .join("Kahawai Server")
                .join("config.toml"))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Some(xdg) = env("XDG_CONFIG_HOME") {
                return Ok(PathBuf::from(xdg).join("kahawai-server/config.toml"));
            }
            let home =
                env("HOME").ok_or_else(|| MusicError::Config("HOME is not set".to_string()))?;
            Ok(PathBuf::from(home).join(".config/kahawai-server/config.toml"))
        }
    }

    /// Resolution order for the config path: an explicit `argv[1]` wins,
    /// then a `config.toml` in the current directory (legacy, pre-wizard
    /// behavior), then the per-OS default under [`Self::default_config_path`].
    pub fn resolve_path(argv1: Option<&Path>) -> Result<PathBuf, MusicError> {
        if let Some(p) = argv1 {
            return Ok(p.to_path_buf());
        }
        let legacy = PathBuf::from("config.toml");
        if legacy.exists() {
            return Ok(legacy);
        }
        Self::default_config_path()
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            music_dirs: Vec::new(),
            audiobook_dirs: Vec::new(),
            bind: default_bind(),
            db_path: default_db_path(),
            preferred_ladder: default_ladder(),
            dsd_story: DsdStory::default(),
            scan_on_startup: false,
            enrichment_enabled: false,
            online_sources_enabled: false,
            radio_browser_url: None,
            enrichment_min_confidence: default_min_confidence(),
            transcode_cache_mb: default_transcode_cache_mb(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_with_all_fields() {
        let toml = r#"
            music_dirs = ["/mnt/music", "/mnt/more-music"]
            bind = "192.168.1.10:9000"
            db_path = "/var/lib/music/catalog.db"
            preferred_ladder = ["passthrough", "flac", "opus"]
            dsd_story = "native"
            scan_on_startup = true
        "#;
        let cfg: ServerConfig = toml::from_str(toml).expect("parse");
        assert_eq!(
            cfg.music_dirs,
            vec![
                PathBuf::from("/mnt/music"),
                PathBuf::from("/mnt/more-music")
            ]
        );
        assert_eq!(cfg.bind, "192.168.1.10:9000");
        assert_eq!(cfg.db_path, PathBuf::from("/var/lib/music/catalog.db"));
        assert_eq!(
            cfg.preferred_ladder,
            vec![
                StreamFormat::Passthrough,
                StreamFormat::Flac,
                StreamFormat::Opus
            ]
        );
        assert_eq!(cfg.dsd_story, DsdStory::Native);
        assert!(cfg.scan_on_startup);
        assert_eq!(cfg.transcode_cache_mb, 8192, "absent → the 8 GiB default");

        // And back to TOML without loss.
        let ser = toml::to_string(&cfg).expect("serialize");
        let back: ServerConfig = toml::from_str(&ser).expect("re-parse");
        assert_eq!(back.bind, cfg.bind);
        assert_eq!(back.music_dirs, cfg.music_dirs);
        assert_eq!(back.db_path, cfg.db_path);
        assert_eq!(back.preferred_ladder, cfg.preferred_ladder);
        assert_eq!(back.dsd_story, cfg.dsd_story);
        assert_eq!(back.scan_on_startup, cfg.scan_on_startup);
    }

    #[test]
    fn empty_toml_yields_sane_defaults() {
        let cfg: ServerConfig = toml::from_str("").expect("parse");
        assert!(cfg.music_dirs.is_empty());
        assert_eq!(cfg.bind, "0.0.0.0:8080");
        assert_eq!(cfg.db_path, PathBuf::from("data/music.db"));
        assert_eq!(
            cfg.preferred_ladder,
            vec![StreamFormat::Passthrough, StreamFormat::Flac]
        );
        assert_eq!(cfg.dsd_story, DsdStory::Pcm);
        assert!(!cfg.scan_on_startup);
    }

    #[test]
    fn load_missing_file_is_an_io_error() {
        let err = ServerConfig::load("/nonexistent/path/config.toml").unwrap_err();
        assert!(matches!(err, MusicError::Io(_)));
    }

    #[test]
    fn load_malformed_toml_is_a_config_error() {
        let dir = std::env::temp_dir().join("kahawai-core-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "bind = [unclosed").unwrap();
        let err = ServerConfig::load(&path).unwrap_err();
        assert!(matches!(err, MusicError::Config(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_config_from_before_audiobooks_loads_with_no_audiobook_folders() {
        let cfg: ServerConfig =
            toml::from_str("music_dirs = [\"/m\"]\nbind = \"0.0.0.0:8080\"\n").expect("parse");
        assert!(cfg.audiobook_dirs.is_empty());
    }

    #[test]
    fn save_round_trips_and_creates_parents() {
        let dir = std::env::temp_dir().join("kahawai-core-config-save-test");
        let path = dir.join("nested/config.toml");
        let cfg = ServerConfig {
            music_dirs: vec![PathBuf::from("/mnt/music")],
            audiobook_dirs: vec![PathBuf::from("/mnt/books")],
            bind: "127.0.0.1:9090".to_string(),
            ..Default::default()
        };
        cfg.save(&path).expect("save");
        let back = ServerConfig::load(&path).expect("load");
        assert_eq!(back.music_dirs, cfg.music_dirs);
        assert_eq!(back.audiobook_dirs, cfg.audiobook_dirs);
        assert_eq!(back.bind, cfg.bind);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_config_path_macos_uses_application_support() {
        let path = ServerConfig::default_config_path_from_env(|k| match k {
            "HOME" => Some("/Users/kyo".to_string()),
            _ => None,
        });
        #[cfg(target_os = "macos")]
        assert_eq!(
            path.unwrap(),
            PathBuf::from("/Users/kyo/Library/Application Support/Kahawai Server/config.toml")
        );
        #[cfg(not(target_os = "macos"))]
        let _ = path; // exercised on non-mac targets by the other branches below
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn default_config_path_linux_prefers_xdg_config_home() {
        let path = ServerConfig::default_config_path_from_env(|k| match k {
            "XDG_CONFIG_HOME" => Some("/tmp/xdg".to_string()),
            "HOME" => Some("/home/kyo".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(path, PathBuf::from("/tmp/xdg/kahawai-server/config.toml"));
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn default_config_path_linux_falls_back_to_home_dot_config() {
        let path = ServerConfig::default_config_path_from_env(|k| match k {
            "HOME" => Some("/home/kyo".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/kyo/.config/kahawai-server/config.toml")
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn default_config_path_windows_uses_appdata() {
        let path = ServerConfig::default_config_path_from_env(|k| match k {
            "APPDATA" => Some(r"C:\Users\kyo\AppData\Roaming".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\kyo\AppData\Roaming\Kahawai Server\config.toml")
        );
    }

    #[test]
    fn default_config_path_missing_home_is_a_config_error() {
        let err = ServerConfig::default_config_path_from_env(|_| None).unwrap_err();
        assert!(matches!(err, MusicError::Config(_)));
    }

    #[test]
    fn resolve_path_prefers_explicit_argv() {
        let explicit = PathBuf::from("/explicit/config.toml");
        let resolved = ServerConfig::resolve_path(Some(&explicit)).unwrap();
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn resolve_path_prefers_legacy_cwd_config_over_default() {
        // No other test in this crate reads or changes the process cwd, so
        // this swap-and-restore is safe under `cargo test`'s parallel runner.
        let dir = std::env::temp_dir().join("kahawai-core-config-resolve-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "bind = \"1.2.3.4:1\"").unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let resolved = ServerConfig::resolve_path(None);
        std::env::set_current_dir(&original_cwd).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(resolved.unwrap(), PathBuf::from("config.toml"));
    }
}
