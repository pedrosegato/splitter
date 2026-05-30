use std::sync::Arc;
use tauri::State;
use splitter_core::net::discovery::DiscoveredPeer;
use crate::core::AppCore;

#[tauri::command]
#[specta::specta]
pub async fn discovered_peers(core: State<'_, Arc<AppCore>>) -> Result<Vec<DiscoveredPeer>, String> {
    Ok(core.peers.read().await.values().cloned().collect())
}
