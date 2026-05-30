use splitter_core::audio::devices::{list_devices as core_list, DeviceInfo};

#[tauri::command]
#[specta::specta]
pub fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    core_list().map_err(|e| e.to_string())
}
