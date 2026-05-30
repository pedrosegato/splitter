use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use splitter_core::net::discovery::DiscoveredPeer;
use splitter_core::net::signaling::{DeviceDescriptor, SignalingMessage, StreamAction};
use crate::core::AppCore;
use crate::dto::{IdentityDto, PendingPeerDto};

#[tauri::command]
#[specta::specta]
pub async fn identity(core: State<'_, Arc<AppCore>>) -> Result<IdentityDto, String> {
    let id = core.identity.read().unwrap().clone();
    Ok(IdentityDto {
        peer_id: id.peer_id.to_string(),
        peer_name: id.peer_name,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn discovered_peers(core: State<'_, Arc<AppCore>>) -> Result<Vec<DiscoveredPeer>, String> {
    Ok(core.peers.read().await.values().cloned().collect())
}

#[tauri::command]
#[specta::specta]
pub async fn pending_peers(core: State<'_, Arc<AppCore>>) -> Result<Vec<PendingPeerDto>, String> {
    Ok(core.server.pending.list().await.iter().map(PendingPeerDto::from).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn accept_pending(core: State<'_, Arc<AppCore>>, index: u32) -> Result<String, String> {
    let (peer_id, _token) = splitter_core::net::signaling::server::accept_pending(
        &core.server.pending,
        &core.trust,
        &core.server.connections,
        &core.server.connection_established_tx,
        index as usize,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(peer_id.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn connect_peer(core: State<'_, Arc<AppCore>>, host: String, port: u16, peer_id: Option<String>) -> Result<bool, String> {
    let addr = format!("{host}:{port}").parse().map_err(|_| format!("invalid address '{host}:{port}'"))?;
    let hint = match peer_id {
        Some(s) => Some(uuid::Uuid::parse_str(&s).map_err(|e| e.to_string())?),
        None => None,
    };
    let identity = core.identity.read().unwrap().clone();
    let outcome = splitter_core::net::signaling::client::connect_to_peer(
        addr,
        &identity,
        core.trust.clone(),
        hint,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| e.to_string())?;
    if let Some(pid) = outcome.remote_peer_id {
        let events = outcome.handle.events.subscribe();
        let addr = outcome.handle.remote_addr;
        let tx = outcome.handle.tx.clone();
        core.outgoing.write().await.insert(pid, outcome.handle);
        crate::acceptor::spawn_acceptor((*core).clone(), pid, events, addr);
        tx.send(SignalingMessage::DeviceListRequest {}).await.ok();
    }
    Ok(outcome.accepted)
}

#[tauri::command]
#[specta::specta]
pub async fn peer_devices(
    core: State<'_, Arc<AppCore>>,
    peer_id: String,
) -> Result<Vec<DeviceDescriptor>, String> {
    let pid = uuid::Uuid::parse_str(&peer_id).map_err(|e| e.to_string())?;
    let cached = core.remote_devices.read().await.get(&pid).cloned();
    if cached.is_none() {
        let g = core.server.connections.read().await;
        let tx = g.get(&pid).map(|c| c.tx.clone());
        drop(g);
        let tx = if let Some(t) = tx {
            Some(t)
        } else {
            core.outgoing.read().await.get(&pid).map(|c| c.tx.clone())
        };
        if let Some(tx) = tx {
            tx.send(SignalingMessage::DeviceListRequest {}).await.ok();
        }
    }
    Ok(cached.unwrap_or_default())
}

pub(crate) async fn teardown_session(core: &AppCore, sid: uuid::Uuid) -> Result<(), String> {
    let snap = core.sessions.snapshot().await;
    if let Some(sess) = snap.iter().find(|s| s.id == sid) {
        for stream in &sess.streams {
            if let Err(e) = core.stream_registry.close(&sid, stream.id).await {
                tracing::warn!(%sid, stream_id = stream.id, "teardown_session: stream_registry.close error: {e}");
            }
            crate::commands::streams::notify_remote(core, sid, stream.id, StreamAction::Close, None).await;
        }
    }
    core.sessions.close(&sid).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn disconnect(core: State<'_, Arc<AppCore>>, session_id: String) -> Result<(), String> {
    let sid = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    teardown_session(&core, sid).await
}
