use crate::core::AppCore;
use crate::dto::PendingStreamRequestDto;
use crate::events::{SnapshotChanged, StreamRequestsChanged};
use crate::stream_requests::PendingStreamRequest;
use splitter_core::net::signaling::client_ops::{
    build_stream_route, find_conn, notify_remote_by_session, stream_open_message,
    wait_for_stream_open_ack, ConnEndpoints,
};
use splitter_core::net::signaling::SourceKind as WireSourceKind;
use splitter_core::net::signaling::{SignalingMessage, StreamAction};
use splitter_core::net::stream_runtime::{
    open_stream_as_source, SourceConfig, SourceKind, StreamControlSignal,
};
use splitter_core::{SessionId, SessionSnapshot, StreamId};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use uuid::Uuid;

async fn find_peer_conn(core: &AppCore, peer_id: Uuid) -> Option<ConnEndpoints> {
    find_conn(&core.server.connections, &core.outgoing, peer_id).await
}

pub(crate) async fn notify_remote(core: &AppCore, sid: Uuid, stream_id: u8, action: StreamAction) {
    notify_remote_by_session(
        &core.sessions,
        &core.server.connections,
        &core.outgoing,
        sid,
        stream_id,
        action,
    )
    .await;
}

#[tauri::command]
#[specta::specta]
pub async fn snapshot(core: State<'_, Arc<AppCore>>) -> Result<Vec<SessionSnapshot>, String> {
    let mut sessions = core.sessions.snapshot().await;
    let trust = core.trust.read().await;
    for session in &mut sessions {
        if let Some(peer) = trust.peer_for(&session.remote_peer_id) {
            session.remote_peer_name = peer.peer_name.clone();
        }
    }
    Ok(sessions)
}

#[tauri::command]
#[specta::specta]
pub async fn open_session(
    core: State<'_, Arc<AppCore>>,
    remote_peer_id: String,
) -> Result<String, String> {
    let remote = Uuid::parse_str(&remote_peer_id).map_err(|e| e.to_string())?;
    let local_peer_id = core.identity.read().peer_id;
    let sid = core.sessions.open_outgoing(local_peer_id, remote).await;
    core.sessions
        .accept(&sid)
        .await
        .map_err(|e| e.to_string())?;
    let conn = find_peer_conn(&core, remote)
        .await
        .ok_or_else(|| "no live signaling connection to remote peer".to_string())?;
    core.sessions
        .set_session_owner(&sid, conn.connection_id)
        .await;
    conn.tx
        .send(SignalingMessage::SessionRequest {
            session_id: sid.to_string(),
            requested_by: local_peer_id.to_string(),
        })
        .await
        .map_err(|e| e.to_string())?;
    conn.tx
        .send(SignalingMessage::DeviceListRequest {})
        .await
        .ok();
    Ok(sid.to_string())
}

pub(crate) async fn open_stream_core(
    core: &AppCore,
    sid: Uuid,
    source_device_id: String,
    source_is_system: bool,
    sink_peer: Uuid,
    sink_device_id: String,
    bitrate: u32,
) -> Result<u8, String> {
    let sid = SessionId(sid);
    let local_peer_id = core.identity.read().peer_id;

    let session = core
        .sessions
        .snapshot()
        .await
        .into_iter()
        .find(|s| s.id == sid)
        .ok_or_else(|| format!("session {sid} not found"))?;
    if session.remote_peer_id != sink_peer {
        return Err(format!("session {sid} is not bound to peer {sink_peer}"));
    }
    let stream_id: u8 = core
        .sessions
        .next_stream_id(&sid)
        .await
        .map_err(|e| e.to_string())?
        .get();

    let conn = find_peer_conn(core, sink_peer)
        .await
        .ok_or_else(|| "no live signaling connection to remote peer".to_string())?;

    let mut ack_rx = conn.events.subscribe();
    let (open_msg, keys) = stream_open_message(
        conn.require_session_keys().map_err(|e| e.to_string())?,
        sid.get(),
        stream_id,
        local_peer_id,
        sink_peer,
        &source_device_id,
        &sink_device_id,
        bitrate,
    );
    conn.tx.send(open_msg).await.map_err(|e| e.to_string())?;

    let ack_port = wait_for_stream_open_ack(&mut ack_rx, stream_id, Duration::from_secs(5))
        .await
        .map_err(|e| {
            use splitter_core::error::NetError;
            match e {
                NetError::SignalingProtocol { .. } => {
                    "the other PC rejected the stream".to_string()
                }
                NetError::Timeout { .. } => "timed out waiting for stream_open_ack".to_string(),
                other => other.to_string(),
            }
        })?;
    let remote: SocketAddr = SocketAddr::new(conn.remote_addr.ip(), ack_port);

    let route = build_stream_route(
        local_peer_id,
        sink_peer,
        &source_device_id,
        &sink_device_id,
        bitrate,
    );
    let source_kind = if source_is_system {
        SourceKind::System
    } else {
        SourceKind::Mic(source_device_id)
    };
    let session_route = route.clone();
    let config = SourceConfig::from_settings(&*core.settings.read().await, bitrate, Some(keys));
    open_stream_as_source(
        core.stream_registry.clone(),
        sid,
        StreamId(stream_id),
        route,
        remote,
        source_kind,
        config,
    )
    .await
    .map_err(|e| e.to_string())?;
    let stream = splitter_core::net::stream::Stream::new_negotiating(
        StreamId(stream_id),
        session_route,
        ack_port,
    );
    if let Err(e) = core.sessions.add_stream(&sid, stream).await {
        let _ = core.stream_registry.close(&sid, StreamId(stream_id)).await;
        return Err(e.to_string());
    }
    if let Err(e) = core
        .sessions
        .activate_stream(&sid, StreamId(stream_id))
        .await
    {
        let _ = core.stream_registry.close(&sid, StreamId(stream_id)).await;
        let _ = core.sessions.remove_stream(&sid, StreamId(stream_id)).await;
        return Err(e.to_string());
    }
    tracing::info!(%sid, stream_id, "stream now active");
    Ok(stream_id)
}

#[tauri::command]
#[specta::specta]
pub async fn open_stream(
    core: State<'_, Arc<AppCore>>,
    session_id: String,
    source_device_id: String,
    source_is_system: bool,
    sink_peer_id: String,
    sink_device_id: String,
    bitrate: Option<u32>,
) -> Result<u8, String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let bitrate = match bitrate {
        Some(b) => b,
        None => core.settings.read().await.default_bitrate,
    };
    let remote_peer_id = core
        .sessions
        .snapshot()
        .await
        .iter()
        .find(|s| s.id.get() == sid)
        .map(|s| s.remote_peer_id)
        .ok_or_else(|| format!("session {sid} not found"))?;
    let sink_uuid = Uuid::parse_str(&sink_peer_id).map_err(|e| e.to_string())?;
    if sink_uuid != remote_peer_id {
        return Err(format!(
            "sink peer {sink_uuid} does not match session remote {remote_peer_id}"
        ));
    }
    open_stream_core(
        &core,
        sid,
        source_device_id,
        source_is_system,
        remote_peer_id,
        sink_device_id,
        bitrate,
    )
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn request_stream(
    core: State<'_, Arc<AppCore>>,
    session_id: String,
    source: WireSourceKind,
    sink_device_id: String,
) -> Result<(), String> {
    let sid = SessionId(Uuid::parse_str(&session_id).map_err(|e| e.to_string())?);
    let remote = core
        .sessions
        .snapshot()
        .await
        .iter()
        .find(|s| s.id == sid)
        .map(|s| s.remote_peer_id)
        .ok_or_else(|| format!("session {sid} not found"))?;
    let conn = find_peer_conn(&core, remote)
        .await
        .ok_or_else(|| "no live signaling connection to remote peer".to_string())?;
    conn.tx
        .send(SignalingMessage::StreamRequest {
            session_id: sid.to_string(),
            source,
            sink_device: sink_device_id,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn take_stream_request(core: &AppCore, id: Uuid) -> Result<PendingStreamRequest, String> {
    let mut requests = core.stream_requests.write().await;
    let idx = requests
        .iter()
        .position(|r| r.id == id)
        .ok_or_else(|| format!("stream request {id} not found"))?;
    Ok(requests.remove(idx))
}

pub(crate) async fn reject_stream_request_core(core: &AppCore, id: Uuid) -> Result<(), String> {
    take_stream_request(core, id).await?;
    core.emit(StreamRequestsChanged);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn pending_stream_requests(
    core: State<'_, Arc<AppCore>>,
) -> Result<Vec<PendingStreamRequestDto>, String> {
    let requests = core.stream_requests.read().await.clone();
    let mut out = Vec::with_capacity(requests.len());
    for r in &requests {
        let device_id = r.source_device_id();
        out.push(PendingStreamRequestDto {
            id: r.id.to_string(),
            peer_id: r.peer_id.to_string(),
            peer_name: core.peer_display_name(r.peer_id).await,
            source_is_system: r.source_is_system(),
            source_device_name: device_id
                .splitn(3, ':')
                .nth(2)
                .unwrap_or(device_id)
                .to_string(),
        });
    }
    Ok(out)
}

#[tauri::command]
#[specta::specta]
pub async fn accept_stream_request(
    core: State<'_, Arc<AppCore>>,
    request_id: String,
) -> Result<u8, String> {
    let id = Uuid::parse_str(&request_id).map_err(|e| e.to_string())?;
    let request = take_stream_request(&core, id).await?;
    core.emit(StreamRequestsChanged);
    let source_is_system = request.source_is_system();
    let bitrate = core.settings.read().await.default_bitrate;
    let stream_id = open_stream_core(
        &core,
        request.session_id,
        request.source_device_id().to_string(),
        source_is_system,
        request.peer_id,
        request.sink_device,
        bitrate,
    )
    .await?;
    core.emit(SnapshotChanged);
    Ok(stream_id)
}

#[tauri::command]
#[specta::specta]
pub async fn reject_stream_request(
    core: State<'_, Arc<AppCore>>,
    request_id: String,
) -> Result<(), String> {
    let id = Uuid::parse_str(&request_id).map_err(|e| e.to_string())?;
    reject_stream_request_core(&core, id).await
}

#[tauri::command]
#[specta::specta]
pub async fn close_stream(
    core: State<'_, Arc<AppCore>>,
    session_id: String,
    stream_id: u8,
) -> Result<(), String> {
    let sid = SessionId(Uuid::parse_str(&session_id).map_err(|e| e.to_string())?);
    close_stream_core(&core, sid, stream_id).await
}

async fn close_stream_core(core: &AppCore, sid: SessionId, stream_id: u8) -> Result<(), String> {
    core.stream_registry
        .close(&sid, StreamId(stream_id))
        .await
        .map_err(|e| e.to_string())?;
    let _ = core.sessions.remove_stream(&sid, StreamId(stream_id)).await;
    notify_remote(core, sid.get(), stream_id, StreamAction::Close).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn stream_control(
    core: State<'_, Arc<AppCore>>,
    session_id: String,
    stream_id: u8,
    action: StreamAction,
) -> Result<(), String> {
    let sid = SessionId(Uuid::parse_str(&session_id).map_err(|e| e.to_string())?);
    stream_control_core(&core, sid, stream_id, action).await
}

async fn stream_control_core(
    core: &AppCore,
    sid: SessionId,
    stream_id: u8,
    action: StreamAction,
) -> Result<(), String> {
    let signal = StreamControlSignal::from(action.clone());
    if matches!(signal, StreamControlSignal::Close) {
        return close_stream_core(core, sid, stream_id).await;
    }
    if let StreamControlSignal::SetMuted(m) = signal {
        core.sessions
            .set_stream_muted(&sid, StreamId(stream_id), m)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let StreamControlSignal::SetVolume(v) = signal {
        core.sessions
            .set_stream_volume(&sid, StreamId(stream_id), v)
            .await
            .map_err(|e| e.to_string())?;
    }
    core.stream_registry
        .send_control(&sid, StreamId(stream_id), signal)
        .await
        .map_err(|e| e.to_string())?;
    notify_remote(core, sid.get(), stream_id, action).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_action_into_signal_set_volume() {
        assert_eq!(
            StreamControlSignal::from(StreamAction::SetVolume { volume: 0.5 }),
            StreamControlSignal::SetVolume(0.5)
        );
    }

    #[test]
    fn stream_action_into_signal_set_muted() {
        assert_eq!(
            StreamControlSignal::from(StreamAction::SetMuted { muted: true }),
            StreamControlSignal::SetMuted(true)
        );
        assert_eq!(
            StreamControlSignal::from(StreamAction::SetMuted { muted: false }),
            StreamControlSignal::SetMuted(false)
        );
    }

    #[test]
    fn stream_action_into_signal_pause_resume_close() {
        assert_eq!(
            StreamControlSignal::from(StreamAction::Pause),
            StreamControlSignal::Pause
        );
        assert_eq!(
            StreamControlSignal::from(StreamAction::Resume),
            StreamControlSignal::Resume
        );
        assert_eq!(
            StreamControlSignal::from(StreamAction::Close),
            StreamControlSignal::Close
        );
    }

    #[test]
    fn session_snapshot_serializes_to_json() {
        use splitter_core::net::session::SessionState;
        let snap = SessionSnapshot {
            id: SessionId(Uuid::nil()),
            remote_peer_id: Uuid::nil(),
            remote_peer_name: String::new(),
            state: SessionState::Active,
            streams: vec![],
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"state\":\"active\""));
        assert!(json.contains("\"streams\":[]"));
    }

    async fn new_core() -> Arc<AppCore> {
        use tempfile::tempdir;
        AppCore::init(tempdir().unwrap().path())
            .await
            .expect("init")
    }

    #[tokio::test]
    async fn open_stream_core_session_not_found_errors() {
        let core = new_core().await;
        let err = open_stream_core(
            &core,
            Uuid::new_v4(),
            "mic".into(),
            false,
            Uuid::new_v4(),
            "spk".into(),
            64_000,
        )
        .await
        .expect_err("missing session must error");
        assert!(err.contains("not found"));
    }

    #[tokio::test]
    async fn open_stream_core_session_not_bound_to_peer_errors() {
        let core = new_core().await;
        let local = core.identity.read().peer_id;
        let peer_a = Uuid::new_v4();
        let sid = core.sessions.open_outgoing(local, peer_a).await;
        core.sessions.accept(&sid).await.unwrap();

        let peer_b = Uuid::new_v4();
        let err = open_stream_core(
            &core,
            sid.get(),
            "mic".into(),
            false,
            peer_b,
            "spk".into(),
            64_000,
        )
        .await
        .expect_err("session bound to a different peer must error");
        assert!(err.contains("not bound to peer"));
    }

    #[tokio::test]
    async fn notify_remote_no_session_is_noop() {
        let core = new_core().await;
        notify_remote(&core, Uuid::new_v4(), 0, StreamAction::Close).await;
    }

    #[tokio::test]
    async fn stream_control_close_tears_down_like_close_stream() {
        use splitter_core::net::signaling::{Codec, CodecParams, Endpoint};
        use splitter_core::net::stream::{Stream, StreamRoute};
        use splitter_core::net::stream_runtime::{DeviceGuard, StreamRuntime, StreamStats};

        let core = new_core().await;
        let local = core.identity.read().peer_id;
        let sid = core.sessions.open_outgoing(local, Uuid::new_v4()).await;
        core.sessions.accept(&sid).await.unwrap();
        let route = StreamRoute::new(
            Endpoint {
                peer_id: local.to_string(),
                device_id: "in".into(),
            },
            Endpoint {
                peer_id: "remote".into(),
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
        let (control_tx, _control_rx) = tokio::sync::mpsc::channel(4);
        core.stream_registry
            .register(StreamRuntime {
                session_id: sid,
                stream_id: StreamId(0),
                stats: Arc::new(StreamStats::default()),
                control_tx,
                bound_device_id: None,
                join: tokio::spawn(async {}),
                device_guard: DeviceGuard::None,
            })
            .await
            .unwrap();

        stream_control_core(&core, sid, 0, StreamAction::Close)
            .await
            .unwrap();

        assert!(core.stream_registry.list().await.is_empty());
        let snap = core.sessions.snapshot().await;
        assert!(
            snap.iter()
                .find(|s| s.id == sid)
                .unwrap()
                .streams
                .is_empty(),
            "Close via stream_control must drop the stream from the session too"
        );
    }
}
