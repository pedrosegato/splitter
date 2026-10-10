use crate::core::AppCore;
use splitter_core::settings::settings_path;
use splitter_core::{FecMode, JitterMode, LogLevel, Settings};
use std::sync::Arc;
use tauri::State;

/// Leaves `s` untouched unless the result passes `Settings::validate`: AppCore::init
/// refuses to start on a settings file that fails it.
pub fn apply_setting(s: &mut Settings, key: &str, val: &str) -> Result<(), String> {
    let mut next = s.clone();
    set_field(&mut next, key, val)?;
    next.validate().map_err(|e| e.to_string())?;
    *s = next;
    Ok(())
}

fn set_field(s: &mut Settings, key: &str, val: &str) -> Result<(), String> {
    match key {
        "auto_accept_trusted" => {
            s.auto_accept_trusted = val.parse().map_err(|_| "expected bool")?
        }
        "auto_start_with_system" => {
            s.auto_start_with_system = val.parse().map_err(|_| "expected bool")?
        }
        "default_bitrate" => s.default_bitrate = val.parse().map_err(|_| "expected u32")?,
        "fec_mode" => {
            s.fec_mode = match val {
                "auto" => FecMode::Auto,
                "always" => FecMode::Always,
                "never" => FecMode::Never,
                _ => return Err("auto|always|never".into()),
            }
        }
        "jitter_mode" => {
            s.jitter_mode = match val {
                "auto" => JitterMode::Auto,
                "min" => JitterMode::Min,
                v if v.starts_with("fixed:") => {
                    let ms: u32 = v[6..].parse().map_err(|_| "fixed:<ms> expected u32")?;
                    JitterMode::Fixed(ms)
                }
                _ => return Err("auto|min|fixed:<ms>".into()),
            }
        }
        "jitter_max_depth_ms" => s.jitter_max_depth_ms = val.parse().map_err(|_| "expected u32")?,
        "log_level" => {
            s.log_level = match val {
                "trace" => LogLevel::Trace,
                "debug" => LogLevel::Debug,
                "info" => LogLevel::Info,
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                _ => return Err("trace|debug|info|warn|error".into()),
            }
        }
        "signaling_port" => s.signaling_port = val.parse().map_err(|_| "expected u16")?,
        other => return Err(format!("unknown setting: {other}")),
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn settings_get(core: State<'_, Arc<AppCore>>) -> Result<Settings, String> {
    Ok(core.settings.read().await.clone())
}

#[tauri::command]
#[specta::specta]
pub async fn settings_set(
    core: State<'_, Arc<AppCore>>,
    key: String,
    value: String,
) -> Result<Settings, String> {
    let mut guard = core.settings.write().await;
    apply_setting(&mut guard, &key, &value)?;
    guard
        .save_atomic(&settings_path().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(guard.clone())
}

pub fn reset_settings(_current: Settings) -> Settings {
    Settings::default()
}

#[tauri::command]
#[specta::specta]
pub async fn settings_reset(core: State<'_, Arc<AppCore>>) -> Result<Settings, String> {
    let mut guard = core.settings.write().await;
    *guard = reset_settings(guard.clone());
    guard
        .save_atomic(&settings_path().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(guard.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::Settings;

    #[test]
    fn apply_field_sets_log_level_and_bitrate() {
        let mut s = Settings::default();
        apply_setting(&mut s, "log_level", "debug").unwrap();
        apply_setting(&mut s, "default_bitrate", "96000").unwrap();
        apply_setting(&mut s, "jitter_mode", "fixed:40").unwrap();
        assert!(matches!(s.log_level, splitter_core::LogLevel::Debug));
        assert_eq!(s.default_bitrate, 96000);
        assert!(matches!(
            s.jitter_mode,
            splitter_core::JitterMode::Fixed(40)
        ));
        assert!(apply_setting(&mut s, "nope", "x").is_err());
        assert!(apply_setting(&mut s, "jitter_mode", "fixed:xx").is_err());
    }

    #[test]
    fn apply_signaling_port_parses_u16() {
        let mut s = Settings::default();
        apply_setting(&mut s, "signaling_port", "8080").unwrap();
        assert_eq!(s.signaling_port, 8080);
        assert!(apply_setting(&mut s, "signaling_port", "not_a_number").is_err());
    }

    #[test]
    fn apply_rejects_value_that_fails_validation_and_leaves_settings_untouched() {
        let mut s = Settings::default();
        // 9000 collides with the default metrics_port.
        assert!(apply_setting(&mut s, "signaling_port", "9000").is_err());
        assert!(apply_setting(&mut s, "default_bitrate", "5000").is_err());
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn the_app_cannot_turn_on_the_metrics_endpoint() {
        let mut s = Settings::default();
        assert!(apply_setting(&mut s, "metrics_enabled", "true").is_err());
        assert!(!s.metrics_enabled);
    }

    #[test]
    fn reset_replaces_with_default() {
        let s = Settings {
            default_bitrate: 128_000,
            metrics_enabled: true,
            ..Default::default()
        };
        let s = reset_settings(s);
        assert_eq!(s, Settings::default());
    }
}
