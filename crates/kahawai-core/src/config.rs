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
    /// Preferred format ladder, first satisfiable entry wins
    /// (see [`crate::format::transcode_ladder`]).
    #[serde(default = "default_ladder")]
    pub preferred_ladder: Vec<StreamFormat>,
    #[serde(default)]
    pub dsd_story: DsdStory,
    /// Run the library scanner once at startup. Default off.
    #[serde(default)]
    pub scan_on_startup: bool,
}

impl ServerConfig {
    /// Load from a TOML file. Missing file / parse error → caller decides
    /// (the server falls back to [`ServerConfig::default`] with a warning).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, MusicError> {
        let text = std::fs::read_to_string(path.as_ref()).map_err(MusicError::Io)?;
        toml::from_str(&text).map_err(|e| MusicError::Config(e.to_string()))
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            music_dirs: Vec::new(),
            bind: default_bind(),
            db_path: default_db_path(),
            preferred_ladder: default_ladder(),
            dsd_story: DsdStory::default(),
            scan_on_startup: false,
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
}
