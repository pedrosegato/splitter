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
use tauri_specta::{collect_commands, collect_events, Builder};
use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};

fn build() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::devices::list_devices,
            commands::settings::settings_get,
            commands::settings::settings_set,
            commands::peers::identity,
            commands::peers::discovered_peers,
            commands::peers::pending_peers,
            commands::peers::connect_peer,
            commands::peers::accept_pending,
            commands::peers::peer_devices,
            commands::peers::disconnect,
            commands::streams::snapshot,
            commands::streams::open_session,
            commands::streams::open_stream,
            commands::streams::close_stream,
            commands::streams::stream_control,
            commands::ops::mute_all,
            commands::ops::disconnect_all,
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
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcut(mute_shortcut)
                .expect("valid mute shortcut")
                .with_shortcut(pause_shortcut)
                .expect("valid pause shortcut")
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
                    let _ = core.app.set(handle);
                    core.spawn_discovery().expect("discovery");
                    core.spawn_stats_emitter();
                    core.spawn_acceptor_supervisor();
                    app.manage(core);
                }
                Err(e) => {
                    tracing::error!("fatal: AppCore init failed: {e}");
                    eprintln!("fatal: Splitter failed to start: {e}");
                    std::process::exit(1);
                }
            }
            tray::build_tray(app.handle())?;
            if let Some(win) = app.get_webview_window("main") {
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
