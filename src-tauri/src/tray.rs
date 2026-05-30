use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use crate::core::AppCore;

pub const TRAY_ID: &str = "splitter-main";

const TRAY_ICON_SIZE: u32 = 22;

static ICON_IDLE: &[u8] = include_bytes!("../icons/tray/idle.rgba");
static ICON_ACTIVE: &[u8] = include_bytes!("../icons/tray/active.rgba");
static ICON_DEGRADED: &[u8] = include_bytes!("../icons/tray/degraded.rgba");
static ICON_ERROR: &[u8] = include_bytes!("../icons/tray/error.rgba");

pub fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let abrir = MenuItem::with_id(app, "abrir", "Abrir", true, None::<&str>)?;
    let mute_all = MenuItem::with_id(app, "mute_all", "Mutar tudo", true, None::<&str>)?;
    let disconnect_all = MenuItem::with_id(app, "disconnect_all", "Desconectar tudo", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sair = MenuItem::with_id(app, "sair", "Sair", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&abrir, &mute_all, &disconnect_all, &sep, &sair])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("app must have a window icon");

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "abrir" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            "sair" => {
                app.exit(0);
            }
            "mute_all" => {
                let core: Arc<AppCore> = app.state::<Arc<AppCore>>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    crate::commands::ops::mute_all_core(&core).await;
                });
            }
            "disconnect_all" => {
                let core: Arc<AppCore> = app.state::<Arc<AppCore>>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    crate::commands::ops::disconnect_all_core(&core).await;
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}

pub fn set_tray_state(app: &tauri::AppHandle, state: &str) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    let bytes = icon_bytes_for_state(state);
    let img = tauri::image::Image::new_owned(
        bytes.to_vec(),
        TRAY_ICON_SIZE,
        TRAY_ICON_SIZE,
    );
    tray.set_icon(Some(img))?;
    Ok(())
}

fn icon_bytes_for_state(state: &str) -> &'static [u8] {
    match state {
        "active" => ICON_ACTIVE,
        "degraded" => ICON_DEGRADED,
        "error" => ICON_ERROR,
        _ => ICON_IDLE,
    }
}
