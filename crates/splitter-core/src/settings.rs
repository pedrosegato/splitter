use crate::error::NetError;
use crate::net::fs_util::{ensure_private_dir, write_atomic};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

pub type SettingsHandle = Arc<RwLock<Settings>>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum FecMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum JitterMode {
    Auto,
    Min,
    Fixed(u32),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
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
    pub signaling_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_accept_trusted: true,
            auto_start_with_system: false,
            default_bitrate: 64_000,
            fec_mode: FecMode::Always,
            fec_on_threshold_pct: 1,
            fec_off_threshold_pct: 0,
            fec_hysteresis_secs: 10,
            jitter_mode: JitterMode::Auto,
            jitter_max_depth_ms: 200,
            log_level: LogLevel::Info,
            metrics_enabled: false,
            metrics_port: 9000,
            signaling_port: 7000,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), NetError> {
        if self.fec_off_threshold_pct > self.fec_on_threshold_pct {
            return Err(NetError::ConfigIo(format!(
                "fec_off_threshold_pct ({}) must be <= fec_on_threshold_pct ({})",
                self.fec_off_threshold_pct, self.fec_on_threshold_pct
            )));
        }
        if self.signaling_port == 0 {
            return Err(NetError::ConfigIo("signaling_port must not be 0".into()));
        }
        if self.metrics_port == 0 {
            return Err(NetError::ConfigIo("metrics_port must not be 0".into()));
        }
        if self.metrics_port == self.signaling_port {
            return Err(NetError::ConfigIo(format!(
                "metrics_port and signaling_port must differ (both are {})",
                self.metrics_port
            )));
        }
        if !(6_000..=510_000).contains(&self.default_bitrate) {
            return Err(NetError::ConfigIo(format!(
                "default_bitrate ({}) must be in 6000..=510000",
                self.default_bitrate
            )));
        }
        if let JitterMode::Fixed(ms) = self.jitter_mode {
            if ms > self.jitter_max_depth_ms {
                return Err(NetError::ConfigIo(format!(
                    "jitter_mode Fixed({ms}) exceeds jitter_max_depth_ms ({})",
                    self.jitter_max_depth_ms
                )));
            }
        }
        Ok(())
    }

    pub fn load_or_default(path: &Path) -> Result<Self, NetError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| NetError::ConfigIo(format!("read {}: {e}", path.display())))?;
        let parsed: Self = toml::from_str(&raw)
            .map_err(|e| NetError::ConfigIo(format!("parse {}: {e}", path.display())))?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn save_atomic(&self, path: &Path) -> Result<(), NetError> {
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)
                .map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", parent.display())))?;
        }
        let raw = toml::to_string_pretty(self)
            .map_err(|e| NetError::ConfigIo(format!("serialize settings: {e}")))?;
        write_atomic(path, raw.as_bytes())
            .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", path.display())))?;
        Ok(())
    }
}

pub fn settings_path() -> Result<PathBuf, NetError> {
    let base = dirs::config_dir()
        .ok_or_else(|| NetError::ConfigIo("no config_dir available on this platform".into()))?;
    Ok(base.join("Splitter").join("settings.toml"))
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
            signaling_port: 8888,
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

    #[test]
    fn signaling_port_defaults_to_7000_and_roundtrips() {
        let s = Settings::default();
        assert_eq!(s.signaling_port, 7000);
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let custom = Settings {
            signaling_port: 8765,
            ..Default::default()
        };
        custom.save_atomic(&path).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(loaded.signaling_port, 8765);
    }

    #[test]
    fn signaling_port_missing_in_file_falls_back_to_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "metrics_enabled = true\n").unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(loaded.signaling_port, 7000);
    }

    #[test]
    fn validate_default_is_ok() {
        assert!(Settings::default().validate().is_ok());
    }

    #[test]
    fn validate_fec_off_greater_than_on_is_err() {
        let s = Settings {
            fec_on_threshold_pct: 2,
            fec_off_threshold_pct: 5,
            ..Default::default()
        };
        let err = s.validate().unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }

    #[test]
    fn validate_fec_off_equal_to_on_is_ok() {
        let s = Settings {
            fec_on_threshold_pct: 3,
            fec_off_threshold_pct: 3,
            ..Default::default()
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_signaling_port_zero_is_err() {
        let s = Settings {
            signaling_port: 0,
            ..Default::default()
        };
        let err = s.validate().unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }

    #[test]
    fn validate_metrics_port_zero_is_err() {
        let s = Settings {
            metrics_port: 0,
            ..Default::default()
        };
        let err = s.validate().unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }

    #[test]
    fn validate_ports_equal_is_err() {
        let s = Settings {
            signaling_port: 8000,
            metrics_port: 8000,
            ..Default::default()
        };
        let err = s.validate().unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }

    #[test]
    fn validate_default_bitrate_too_low_is_err() {
        let s = Settings {
            default_bitrate: 5_999,
            ..Default::default()
        };
        let err = s.validate().unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }

    #[test]
    fn validate_default_bitrate_too_high_is_err() {
        let s = Settings {
            default_bitrate: 510_001,
            ..Default::default()
        };
        let err = s.validate().unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }

    #[test]
    fn validate_default_bitrate_boundary_values_are_ok() {
        let lo = Settings {
            default_bitrate: 6_000,
            ..Default::default()
        };
        assert!(lo.validate().is_ok());
        let hi = Settings {
            default_bitrate: 510_000,
            ..Default::default()
        };
        assert!(hi.validate().is_ok());
    }

    #[test]
    fn validate_fixed_jitter_exceeds_max_depth_is_err() {
        let s = Settings {
            jitter_mode: JitterMode::Fixed(300),
            jitter_max_depth_ms: 200,
            ..Default::default()
        };
        let err = s.validate().unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }

    #[test]
    fn validate_fixed_jitter_equal_to_max_depth_is_ok() {
        let s = Settings {
            jitter_mode: JitterMode::Fixed(200),
            jitter_max_depth_ms: 200,
            ..Default::default()
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn load_or_default_rejects_invalid_toml_via_validate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "signaling_port = 9000\nmetrics_port = 9000\n").unwrap();
        let err = Settings::load_or_default(&path).unwrap_err();
        assert!(matches!(err, NetError::ConfigIo(_)));
    }
}
