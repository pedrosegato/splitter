use std::sync::Arc;
use tauri::State;
use splitter_core::net::stream_runtime::StreamControlSignal;
use crate::core::AppCore;

pub(crate) async fn mute_all_core(core: &AppCore) {
    let snap = core.sessions.snapshot().await;
    for sess in &snap {
        for stream in &sess.streams {
            if let Err(e) = core.sessions.set_stream_muted(&sess.id, stream.id, true).await {
                tracing::warn!(sid = %sess.id, stream_id = stream.id, "mute_all: set_stream_muted error: {e}");
            }
            if let Err(e) = core.stream_registry.send_control(&sess.id, stream.id, StreamControlSignal::SetMuted(true)).await {
                tracing::warn!(sid = %sess.id, stream_id = stream.id, "mute_all: send_control error: {e}");
            }
        }
    }
}

pub(crate) async fn pause_all_core(core: &AppCore) {
    let snap = core.sessions.snapshot().await;
    for sess in &snap {
        for stream in &sess.streams {
            if let Err(e) = core.stream_registry.send_control(&sess.id, stream.id, StreamControlSignal::Pause).await {
                tracing::warn!(sid = %sess.id, stream_id = stream.id, "pause_all: send_control error: {e}");
            }
        }
    }
}

pub(crate) async fn disconnect_all_core(core: &AppCore) {
    let snap = core.sessions.snapshot().await;
    for sess in &snap {
        if let Err(e) = crate::commands::peers::teardown_session(core, sess.id).await {
            tracing::warn!(sid = %sess.id, "disconnect_all: teardown_session error: {e}");
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn mute_all(core: State<'_, Arc<AppCore>>) -> Result<(), String> {
    mute_all_core(&core).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn disconnect_all(core: State<'_, Arc<AppCore>>) -> Result<(), String> {
    disconnect_all_core(&core).await;
    Ok(())
}
