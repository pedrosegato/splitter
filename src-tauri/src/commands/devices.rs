use splitter_core::audio::devices::{list_devices as core_list, DeviceInfo};

#[tauri::command]
#[specta::specta]
pub async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    // Sync commands run on the UI thread, and WASAPI enumeration is slow.
    tokio::task::spawn_blocking(core_list)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}
