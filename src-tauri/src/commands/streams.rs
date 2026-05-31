use crate::core::AppCore;
use splitter_core::net::signaling::client_ops::{
    build_stream_route, find_conn, notify_remote_control, stream_open_message,
    wait_for_stream_open_ack, ConnEndpoints,
};
use splitter_core::net::signaling::{SignalingMessage, StreamAction};
use splitter_core::net::stream_runtime::{open_stream_as_source, SourceKind, StreamControlSignal};
use splitter_core::SessionSnapshot;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use uuid::Uuid;

fn signal_from(action: &str, value: Option<f32>) -> Result<StreamControlSignal, String> {
    match action {
        "set_volume" => {
            let v = value.ok_or_else(|| "set_volume requires a value".to_string())?;
            Ok(StreamControlSignal::SetVolume(v))
        }
        "set_muted" => {
            let v = value.ok_or_else(|| "set_muted requires a value".to_string())?;
            Ok(StreamControlSignal::SetMuted(v != 0.0))
        }
        "pause" => Ok(StreamControlSignal::Pause),
        "resume" => Ok(StreamControlSignal::Resume),
        "close" => Ok(StreamControlSignal::Close),
        other => Err(format!("unknown stream action: {other}")),
    }
}

fn remote_action_from(signal: &StreamControlSignal) -> Option<(StreamAction, Option<f32>)> {
    match signal {
        StreamControlSignal::Pause => Some((StreamAction::Pause, None)),
        StreamControlSignal::Resume => Some((StreamAction::Resume, None)),
        StreamControlSignal::Close => Some((StreamAction::Close, None)),
        StreamControlSignal::SetVolume(v) => Some((StreamAction::SetVolume, Some(*v))),
        StreamControlSignal::SetMuted(_) => None,
    }
}

async fn find_peer_conn(core: &AppCore, peer_id: Uuid) -> Option<ConnEndpoints> {
    find_conn(&core.server.connections, &core.outgoing, peer_id).await
}

pub(crate) async fn notify_remote(
    core: &AppCore,
    sid: Uuid,
    stream_id: u8,
    action: StreamAction,
    volume: Option<f32>,
) {
    let snap = core.sessions.snapshot().await;
    let remote = match snap.iter().find(|s| s.id == sid).map(|s| s.remote_peer_id) {
        Some(r) => r,
        None => {
            tracing::warn!(%sid, "notify_remote: session not found, skipping remote signal");
            return;
        }
    };
    match find_peer_conn(core, remote).await {
        Some(conn) => {
            notify_remote_control(&conn.tx, stream_id, action, volume).await;
        }
        None => {
            tracing::warn!(%sid, %remote, "notify_remote: no live connection to remote peer, skipping remote signal");
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn snapshot(core: State<'_, Arc<AppCore>>) -> Result<Vec<SessionSnapshot>, String> {
    Ok(core.sessions.snapshot().await)
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
    bitrate: i32,
) -> Result<u8, String> {
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
        .map_err(|e| e.to_string())?;

    let conn = find_peer_conn(core, sink_peer)
        .await
        .ok_or_else(|| "no live signaling connection to remote peer".to_string())?;

    let mut ack_rx = conn.events.subscribe();
    conn.tx
        .send(stream_open_message(
            sid,
            stream_id,
            local_peer_id,
            sink_peer,
            &source_device_id,
            &sink_device_id,
            bitrate,
        ))
        .await
        .map_err(|e| e.to_string())?;

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
    open_stream_as_source(
        core.stream_registry.clone(),
        sid,
        stream_id,
        route,
        remote,
        source_kind,
    )
    .await
    .map_err(|e| e.to_string())?;
    let stream =
        splitter_core::net::stream::Stream::new_negotiating(stream_id, session_route, ack_port);
    if let Err(e) = core.sessions.add_stream(&sid, stream).await {
        let _ = core.stream_registry.close(&sid, stream_id).await;
        return Err(e.to_string());
    }
    if let Err(e) = core.sessions.activate_stream(&sid, stream_id).await {
        let _ = core.stream_registry.close(&sid, stream_id).await;
        let _ = core.sessions.remove_stream(&sid, stream_id).await;
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
    bitrate: Option<i32>,
) -> Result<u8, String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let bitrate = bitrate.unwrap_or(64_000);
    let remote_peer_id = core
        .sessions
        .snapshot()
        .await
        .iter()
        .find(|s| s.id == sid)
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
    source_device_id: String,
    source_is_system: bool,
    sink_device_id: String,
) -> Result<(), String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
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
            source_device: source_device_id,
            source_is_system,
            sink_device: sink_device_id,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn close_stream(
    core: State<'_, Arc<AppCore>>,
    session_id: String,
    stream_id: u8,
) -> Result<(), String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    core.stream_registry
        .close(&sid, stream_id)
        .await
        .map_err(|e| e.to_string())?;
    let _ = core.sessions.remove_stream(&sid, stream_id).await;
    notify_remote(&core, sid, stream_id, StreamAction::Close, None).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn stream_control(
    core: State<'_, Arc<AppCore>>,
    session_id: String,
    stream_id: u8,
    action: String,
    value: Option<f32>,
) -> Result<(), String> {
    let sid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let signal = signal_from(&action, value)?;
    let remote_action = remote_action_from(&signal);
    if matches!(signal, StreamControlSignal::Close) {
        core.stream_registry
            .close(&sid, stream_id)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        if let StreamControlSignal::SetMuted(m) = signal {
            core.sessions
                .set_stream_muted(&sid, stream_id, m)
                .await
                .map_err(|e| e.to_string())?;
        }
        core.stream_registry
            .send_control(&sid, stream_id, signal)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some((ra, rv)) = remote_action {
        notify_remote(&core, sid, stream_id, ra, rv).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_from_set_volume() {
        assert_eq!(
            signal_from("set_volume", Some(0.5)).unwrap(),
            StreamControlSignal::SetVolume(0.5)
        );
    }

    #[test]
    fn signal_from_set_volume_missing_value_errors() {
        assert!(signal_from("set_volume", None).is_err());
    }

    #[test]
    fn signal_from_set_muted_true() {
        assert_eq!(
            signal_from("set_muted", Some(1.0)).unwrap(),
            StreamControlSignal::SetMuted(true)
        );
    }

    #[test]
    fn signal_from_set_muted_false() {
        assert_eq!(
            signal_from("set_muted", Some(0.0)).unwrap(),
            StreamControlSignal::SetMuted(false)
        );
    }

    #[test]
    fn signal_from_pause() {
        assert_eq!(
            signal_from("pause", None).unwrap(),
            StreamControlSignal::Pause
        );
    }

    #[test]
    fn signal_from_resume() {
        assert_eq!(
            signal_from("resume", None).unwrap(),
            StreamControlSignal::Resume
        );
    }

    #[test]
    fn signal_from_close() {
        assert_eq!(
            signal_from("close", None).unwrap(),
            StreamControlSignal::Close
        );
    }

    #[test]
    fn signal_from_unknown_errors() {
        assert!(signal_from("frobnicate", None).is_err());
    }

    #[test]
    fn remote_action_from_pause() {
        assert_eq!(
            remote_action_from(&StreamControlSignal::Pause),
            Some((StreamAction::Pause, None))
        );
    }

    #[test]
    fn remote_action_from_resume() {
        assert_eq!(
            remote_action_from(&StreamControlSignal::Resume),
            Some((StreamAction::Resume, None))
        );
    }

    #[test]
    fn remote_action_from_close() {
        assert_eq!(
            remote_action_from(&StreamControlSignal::Close),
            Some((StreamAction::Close, None))
        );
    }

    #[test]
    fn remote_action_from_set_volume() {
        assert_eq!(
            remote_action_from(&StreamControlSignal::SetVolume(0.75)),
            Some((StreamAction::SetVolume, Some(0.75)))
        );
    }

    #[test]
    fn remote_action_from_set_muted_is_none() {
        assert_eq!(
            remote_action_from(&StreamControlSignal::SetMuted(true)),
            None
        );
        assert_eq!(
            remote_action_from(&StreamControlSignal::SetMuted(false)),
            None
        );
    }

    #[test]
    fn session_snapshot_serializes_to_json() {
        use splitter_core::net::session::SessionState;
        let snap = SessionSnapshot {
            id: Uuid::nil(),
            remote_peer_id: Uuid::nil(),
            state: SessionState::Active,
            streams: vec![],
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"state\":\"active\""));
        assert!(json.contains("\"streams\":[]"));
    }
}
