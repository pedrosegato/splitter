use crate::core::AppCore;
use splitter_core::net::stream_runtime::StreamControlSignal;
use std::sync::Arc;
use tauri::State;

pub(crate) async fn mute_all_core(core: &AppCore) {
    let snap = core.sessions.snapshot().await;
    for sess in &snap {
        for stream in &sess.streams {
            if let Err(e) = core
                .sessions
                .set_stream_muted(&sess.id, stream.id, true)
                .await
            {
                tracing::warn!(sid = %sess.id, stream_id = %stream.id, "mute_all: set_stream_muted error: {e}");
            }
            if let Err(e) = core
                .stream_registry
                .send_control(&sess.id, stream.id, StreamControlSignal::SetMuted(true))
                .await
            {
                tracing::warn!(sid = %sess.id, stream_id = %stream.id, "mute_all: send_control error: {e}");
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

#[tauri::command]
#[specta::specta]
pub async fn set_tray_state(
    app: tauri::AppHandle<tauri::Wry>,
    state: String,
) -> Result<(), String> {
    #[cfg(desktop)]
    crate::tray::set_tray_state(&app, &state).map_err(|e| e.to_string())?;
    #[cfg(not(desktop))]
    let _ = (app, state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::net::signaling::{Codec, CodecParams, Endpoint};
    use splitter_core::net::stream::{Stream, StreamRoute};
    use splitter_core::{SessionId, SessionState, StreamId};
    use tempfile::tempdir;
    use uuid::Uuid;

    async fn seed_active_session_with_stream(core: &AppCore, remote: Uuid) -> SessionId {
        let local = core.identity.read().peer_id;
        let sid = core.sessions.open_outgoing(local, remote).await;
        core.sessions.accept(&sid).await.unwrap();
        let route = StreamRoute::new(
            Endpoint {
                peer_id: local.to_string(),
                device_id: "in".into(),
            },
            Endpoint {
                peer_id: remote.to_string(),
                device_id: "out".into(),
            },
            CodecParams {
                name: Codec::Opus,
                bitrate: 64_000,
                frame_ms: 20,
            },
            1.0,
        );
        core.sessions
            .add_stream(&sid, Stream::new_negotiating(StreamId(0), route, 0))
            .await
            .unwrap();
        core.sessions
            .activate_stream(&sid, StreamId(0))
            .await
            .unwrap();
        sid
    }

    async fn new_core() -> Arc<AppCore> {
        AppCore::init(tempdir().unwrap().path())
            .await
            .expect("init")
    }

    #[tokio::test]
    async fn mute_all_core_mutes_every_stream() {
        let core = new_core().await;
        seed_active_session_with_stream(&core, Uuid::new_v4()).await;
        seed_active_session_with_stream(&core, Uuid::new_v4()).await;

        mute_all_core(&core).await;

        let snap = core.sessions.snapshot().await;
        let streams: Vec<_> = snap.iter().flat_map(|s| &s.streams).collect();
        assert_eq!(streams.len(), 2);
        assert!(streams.iter().all(|st| st.muted));
    }

    #[tokio::test]
    async fn disconnect_all_core_closes_all_sessions() {
        let core = new_core().await;
        seed_active_session_with_stream(&core, Uuid::new_v4()).await;
        seed_active_session_with_stream(&core, Uuid::new_v4()).await;

        disconnect_all_core(&core).await;

        let snap = core.sessions.snapshot().await;
        assert!(snap.iter().all(|s| s.state == SessionState::Closed));
    }
}
