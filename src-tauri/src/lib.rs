mod acceptor;
mod core;
pub use core::AppCore;
mod commands;
mod dto;
pub mod events;
mod reconnect;

use specta_typescript::Typescript;
use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder};

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

    tauri::Builder::default()
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
                    eprintln!("fatal: AudioMirror failed to start: {e}");
                    std::process::exit(1);
                }
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
