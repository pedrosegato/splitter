mod acceptor;
mod core;
pub use core::AppCore;
mod commands;
mod dto;
pub mod events;
mod reconnect;
mod tray;

use specta_typescript::Typescript;
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};
use tauri_specta::{collect_commands, collect_events, Builder};

fn build() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::devices::list_devices,
            commands::settings::settings_get,
            commands::settings::settings_set,
            commands::settings::settings_reset,
            commands::peers::identity,
            commands::peers::discovered_peers,
            commands::peers::pending_peers,
            commands::peers::connect_peer,
            commands::peers::accept_pending,
            commands::peers::peer_devices,
            commands::peers::disconnect,
            commands::peers::set_device_name,
            commands::streams::snapshot,
            commands::streams::open_session,
            commands::streams::open_stream,
            commands::streams::close_stream,
            commands::streams::stream_control,
            commands::ops::mute_all,
            commands::ops::disconnect_all,
            commands::ops::set_tray_state,
            commands::perms::permission_status,
            commands::perms::request_permission,
            commands::system::set_autostart,
        ])
        .events(collect_events![
            events::PeersChanged,
            events::IncomingSession,
            events::StatsTick,
            events::PeerDisconnected,
        ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _logs_guard = splitter_core::observability::logs::log_dir()
        .and_then(|dir| {
            splitter_core::observability::logs::init(splitter_core::LogLevel::Info, &dir)
        })
        .ok();

    let builder = build();

    #[cfg(debug_assertions)]
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("failed to export typescript bindings");

    let mute_shortcut: Shortcut = "CmdOrCtrl+Shift+M".parse().expect("valid mute shortcut");
    let pause_shortcut: Shortcut = "CmdOrCtrl+Shift+P".parse().expect("valid pause shortcut");
    let mute_id = mute_shortcut.id();
    let pause_id = pause_shortcut.id();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_decorum::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    let id = shortcut.id();
                    let core = app.state::<std::sync::Arc<AppCore>>().inner().clone();
                    if id == mute_id {
                        tauri::async_runtime::spawn(async move {
                            commands::ops::mute_all_core(&core).await;
                        });
                    } else if id == pause_id {
                        tauri::async_runtime::spawn(async move {
                            commands::ops::pause_all_core(&core).await;
                        });
                    }
                })
                .build(),
        )
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let handle = app.handle().clone();
            let config_dir = splitter_core::settings::settings_path()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            std::fs::create_dir_all(&config_dir).ok();
            match tauri::async_runtime::block_on(AppCore::init(&config_dir)) {
                Ok(core) => {
                    let auto_start = tauri::async_runtime::block_on(async {
                        core.settings.read().await.auto_start_with_system
                    });
                    let _ = core.app.set(handle);
                    core.spawn_discovery().expect("discovery");
                    core.spawn_stats_emitter();
                    core.spawn_acceptor_supervisor();
                    app.manage(core);
                    let manager = app.autolaunch();
                    if auto_start {
                        if let Err(e) = manager.enable() {
                            tracing::warn!("autostart reconcile enable failed: {e}");
                        }
                    } else {
                        if let Err(e) = manager.disable() {
                            tracing::warn!("autostart reconcile disable failed: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("fatal: AppCore init failed: {e}");
                    eprintln!("fatal: Splitter failed to start: {e}");
                    std::process::exit(1);
                }
            }
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::GlobalShortcutExt;
                let gs = app.global_shortcut();
                if let Err(e) = gs.register(mute_shortcut) {
                    tracing::warn!("global shortcut Ctrl+Shift+M unavailable: {e}");
                }
                if let Err(e) = gs.register(pause_shortcut) {
                    tracing::warn!("global shortcut Ctrl+Shift+P unavailable: {e}");
                }
            }
            tray::build_tray(app.handle())?;
            if let Some(win) = app.get_webview_window("main") {
                #[cfg(target_os = "macos")]
                {
                    use tauri_plugin_decorum::WebviewWindowExt;
                    let _ = win.set_traffic_lights_inset(15.0, 22.0);
                }
                let win_clone = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = win_clone.hide();
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running splitter");
}

#[cfg(test)]
mod export_test {
    use super::build;
    use specta_typescript::Typescript;

    #[test]
    fn exports_typescript_bindings() {
        build()
            .export(Typescript::default(), "../src/bindings.ts")
            .expect("failed to export typescript bindings");
    }
}
