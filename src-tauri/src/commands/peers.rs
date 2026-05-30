use std::sync::Arc;
use std::time::Duration;
use tauri::State;
use splitter_core::net::discovery::DiscoveredPeer;
use crate::core::AppCore;
use crate::dto::PendingPeerDto;

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
pub async fn accept_pending(core: State<'_, Arc<AppCore>>, index: usize) -> Result<String, String> {
    let (peer_id, _token) = splitter_core::net::signaling::server::accept_pending(
        &core.server.pending,
        &core.trust,
        &core.server.connections,
        &core.server.connection_established_tx,
        index,
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
    let outcome = splitter_core::net::signaling::client::connect_to_peer(
        addr,
        &core.identity,
        core.trust.clone(),
        hint,
        Duration::from_secs(5),
    )
    .await
    .map_err(|e| e.to_string())?;
    if let Some(pid) = outcome.remote_peer_id {
        core.outgoing.write().await.insert(pid, outcome.handle);
    }
    Ok(outcome.accepted)
}

#[tauri::command]
#[specta::specta]
pub async fn disconnect(core: State<'_, Arc<AppCore>>, session_id: String) -> Result<(), String> {
    let sid = uuid::Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    core.sessions.close(&sid).await.map_err(|e| e.to_string())
}
