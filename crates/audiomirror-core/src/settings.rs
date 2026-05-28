use crate::error::NetError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

pub type SettingsHandle = Arc<RwLock<Settings>>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FecMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JitterMode {
    Auto,
    Min,
    Fixed(u32),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub auto_accept_trusted: bool,
    pub auto_start_with_system: bool,
    pub default_bitrate: u32,
    pub fec_mode: FecMode,
    pub fec_on_threshold_pct: u32,
    pub fec_off_threshold_pct: u32,
    pub fec_hysteresis_secs: u32,
    pub jitter_mode: JitterMode,
    pub jitter_max_depth_ms: u32,
    pub log_level: LogLevel,
    pub metrics_enabled: bool,
    pub metrics_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_accept_trusted: true,
            auto_start_with_system: false,
            default_bitrate: 64_000,
            fec_mode: FecMode::Auto,
            fec_on_threshold_pct: 1,
            fec_off_threshold_pct: 0,
            fec_hysteresis_secs: 10,
            jitter_mode: JitterMode::Auto,
            jitter_max_depth_ms: 100,
            log_level: LogLevel::Info,
            metrics_enabled: false,
            metrics_port: 9000,
        }
    }
}

impl Settings {
    pub fn load_or_default(path: &Path) -> Result<Self, NetError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| NetError::ConfigIo(format!("read {}: {e}", path.display())))?;
        let parsed: Self = toml::from_str(&raw)
            .map_err(|e| NetError::ConfigIo(format!("parse {}: {e}", path.display())))?;
        Ok(parsed)
    }

    pub fn save_atomic(&self, path: &Path) -> Result<(), NetError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", parent.display())))?;
        }
        let raw = toml::to_string_pretty(self)
            .map_err(|e| NetError::ConfigIo(format!("serialize settings: {e}")))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, raw)
            .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            NetError::ConfigIo(format!(
                "rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        Ok(())
    }
}

pub fn settings_path() -> Result<PathBuf, NetError> {
    let base = dirs::config_dir()
        .ok_or_else(|| NetError::ConfigIo("no config_dir available on this platform".into()))?;
    Ok(base.join("AudioMirror").join("settings.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_or_default_returns_defaults_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let s = Settings::load_or_default(&path).unwrap();
        assert_eq!(s, Settings::default());
        assert!(!path.exists(), "load_or_default must not write a file");
    }

    #[test]
    fn save_atomic_writes_via_tmp_then_rename() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let s = Settings {
            metrics_enabled: true,
            metrics_port: 9100,
            ..Default::default()
        };
        s.save_atomic(&path).unwrap();
        assert!(path.exists());
        let tmp = path.with_extension("toml.tmp");
        assert!(!tmp.exists(), "tmp file must be cleaned up");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("metrics_enabled = true"));
        assert!(raw.contains("metrics_port = 9100"));
    }

    #[test]
    fn save_then_load_round_trip_preserves_all_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let original = Settings {
            auto_accept_trusted: true,
            auto_start_with_system: true,
            default_bitrate: 96_000,
            fec_mode: FecMode::Always,
            fec_on_threshold_pct: 2,
            fec_off_threshold_pct: 1,
            fec_hysteresis_secs: 20,
            jitter_mode: JitterMode::Fixed(40),
            jitter_max_depth_ms: 120,
            log_level: LogLevel::Debug,
            metrics_enabled: true,
            metrics_port: 9100,
        };
        original.save_atomic(&path).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn unknown_field_in_existing_file_is_tolerated() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "metrics_enabled = true\nfuture_unknown = 42\n").unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert!(loaded.metrics_enabled);
    }
}
