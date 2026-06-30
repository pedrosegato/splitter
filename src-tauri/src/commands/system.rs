use crate::core::AppCore;
use splitter_core::settings::settings_path;
use std::sync::Arc;
use tauri::State;
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
#[specta::specta]
pub async fn set_autostart(
    app: tauri::AppHandle<tauri::Wry>,
    core: State<'_, Arc<AppCore>>,
    enabled: bool,
) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())?;
    } else {
        manager.disable().map_err(|e| e.to_string())?;
    }
    let mut guard = core.settings.write().await;
    guard.auto_start_with_system = enabled;
    guard
        .save_atomic(&settings_path().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(())
}
