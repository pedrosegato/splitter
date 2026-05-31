use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PermStatus {
    Granted,
    Denied,
    Prompt,
    NotApplicable,
}

#[derive(Serialize, Deserialize, specta::Type, Clone, Debug)]
pub struct Permissions {
    pub microphone: PermStatus,
    pub screen: PermStatus,
}

#[tauri::command]
#[specta::specta]
pub async fn permission_status() -> Permissions {
    #[cfg(target_os = "macos")]
    {
        let mic = tauri_plugin_macos_permissions::check_microphone_permission().await;
        let screen = tauri_plugin_macos_permissions::check_screen_recording_permission().await;
        Permissions {
            microphone: if mic {
                PermStatus::Granted
            } else {
                PermStatus::Prompt
            },
            screen: if screen {
                PermStatus::Granted
            } else {
                PermStatus::Prompt
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    Permissions {
        microphone: PermStatus::NotApplicable,
        screen: PermStatus::NotApplicable,
    }
}

#[tauri::command]
#[specta::specta]
pub async fn request_permission(kind: String) -> PermStatus {
    #[cfg(target_os = "macos")]
    {
        match kind.as_str() {
            "microphone" => {
                let _ = tauri_plugin_macos_permissions::request_microphone_permission().await;
                let granted = tauri_plugin_macos_permissions::check_microphone_permission().await;
                if granted {
                    PermStatus::Granted
                } else {
                    PermStatus::Denied
                }
            }
            "screen" => {
                tauri_plugin_macos_permissions::request_screen_recording_permission().await;
                let granted =
                    tauri_plugin_macos_permissions::check_screen_recording_permission().await;
                if granted {
                    PermStatus::Granted
                } else {
                    PermStatus::Denied
                }
            }
            _ => PermStatus::Denied,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = kind;
        PermStatus::NotApplicable
    }
}
