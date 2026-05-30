use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use uuid::Uuid;
use splitter_core::SessionSnapshot;
use splitter_core::net::signaling::{CodecParams, Endpoint, PeerEvent, SignalingMessage};
use splitter_core::net::stream::StreamRoute;
use splitter_core::net::stream_runtime::{open_stream_as_source, SourceKind, StreamControlSignal};
use crate::core::AppCore;

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

async fn find_peer_conn(
    core: &AppCore,
    peer_id: Uuid,
) -> Option<(
    tokio::sync::mpsc::Sender<SignalingMessage>,
    SocketAddr,
    tokio::sync::broadcast::Sender<PeerEvent>,
)> {
    {
        let g = core.server.connections.read().await;
        if let Some(c) = g.get(&peer_id) {
            return Some((c.tx.clone(), c.remote_addr, c.events.clone()));
        }
    }
    {
        let g = core.outgoing.read().await;
        if let Some(c) = g.get(&peer_id) {
            return Some((c.tx.clone(), c.remote_addr, c.events.clone()));
        }
    }
    None
}

async fn wait_for_stream_open_ack(
    events: &tokio::sync::broadcast::Sender<PeerEvent>,
    stream_id: u8,
    timeout: Duration,
) -> Result<u16, String> {
    let mut rx = events.subscribe();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("timed out waiting for stream_open_ack".to_string());
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(PeerEvent::Message(SignalingMessage::StreamOpenAck {
                stream_id: id,
                accepted: true,
                udp_port: Some(port),
            }))) if id == stream_id => return Ok(port),
            Ok(Ok(_)) => continue,
            _ => return Err("timed out waiting for stream_open_ack".to_string()),
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
    let sid = core.sessions.open_outgoing(core.identity.peer_id, remote).await;
    core.sessions.accept(&sid).await.map_err(|e| e.to_string())?;
    let (tx, _, _) = find_peer_conn(&core, remote)
        .await
        .ok_or_else(|| "no live signaling connection to remote peer".to_string())?;
    tx.send(SignalingMessage::SessionRequest {
        session_id: sid.to_string(),
        requested_by: core.identity.peer_id.to_string(),
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(sid.to_string())
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

    let snap = core.sessions.snapshot().await;
    let session = snap
        .iter()
        .find(|s| s.id == sid)
        .ok_or_else(|| format!("session {sid} not found"))?;
    let remote_peer_id = session.remote_peer_id;
    let stream_id: u8 = session.streams.len() as u8;

    let _ = sink_peer_id;

    let (conn_tx, conn_remote_addr, conn_events) = find_peer_conn(&core, remote_peer_id)
        .await
        .ok_or_else(|| "no live signaling connection to remote peer".to_string())?;

    conn_tx
        .send(SignalingMessage::StreamOpen {
            session_id: sid.to_string(),
            stream_id,
            source: Endpoint {
                peer_id: core.identity.peer_id.to_string(),
                device_id: source_device_id.clone(),
            },
            sink: Endpoint {
                peer_id: remote_peer_id.to_string(),
                device_id: sink_device_id.clone(),
            },
            codec: CodecParams {
                name: "opus".into(),
                bitrate,
                frame_ms: 20,
            },
            udp_port: 0,
        })
        .await
        .map_err(|e| e.to_string())?;

    let ack_port =
        wait_for_stream_open_ack(&conn_events, stream_id, Duration::from_secs(5)).await?;
    let remote: SocketAddr = SocketAddr::new(conn_remote_addr.ip(), ack_port);

    let route = StreamRoute {
        source: Endpoint {
            peer_id: core.identity.peer_id.to_string(),
            device_id: source_device_id.clone(),
        },
        sink: Endpoint {
            peer_id: remote_peer_id.to_string(),
            device_id: sink_device_id,
        },
        codec: CodecParams {
            name: "opus".into(),
            bitrate,
            frame_ms: 20,
        },
        volume: 1.0,
    };
    let source_kind = if source_is_system {
        SourceKind::System
    } else {
        SourceKind::Mic(source_device_id)
    };
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
    tracing::info!(%sid, stream_id, "stream now active");
    Ok(stream_id)
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
        .map_err(|e| e.to_string())
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
    if matches!(signal, StreamControlSignal::Close) {
        return core
            .stream_registry
            .close(&sid, stream_id)
            .await
            .map_err(|e| e.to_string());
    }
    core.stream_registry
        .send_control(&sid, stream_id, signal)
        .await
        .map_err(|e| e.to_string())
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
        assert_eq!(signal_from("pause", None).unwrap(), StreamControlSignal::Pause);
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
        assert_eq!(signal_from("close", None).unwrap(), StreamControlSignal::Close);
    }

    #[test]
    fn signal_from_unknown_errors() {
        assert!(signal_from("frobnicate", None).is_err());
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
