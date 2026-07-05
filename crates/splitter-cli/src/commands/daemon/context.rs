use splitter_core::audio::devices::{list_devices, DeviceKind};
use splitter_core::net::discovery::DiscoveredPeer;
use splitter_core::net::signaling::{PeerConnectionHandle, PeerEvent};
use splitter_core::net::stream_runtime::StreamRegistry;
use splitter_core::net::trust::TrustStore;
use splitter_core::{PeerIdentity, SessionManager};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

pub(crate) type PeerConnections = Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>;
pub(crate) type DiscoveredPeers = Arc<RwLock<HashMap<String, DiscoveredPeer>>>;

#[derive(Clone)]
pub(crate) struct DaemonContext {
    pub identity: PeerIdentity,
    pub trust: Arc<RwLock<TrustStore>>,
    pub sessions: Arc<SessionManager>,
    pub stream_registry: Arc<StreamRegistry>,
    pub discovered: DiscoveredPeers,
    pub outgoing_connections: PeerConnections,
    pub local_peer_id: Uuid,
}

impl DaemonContext {
    pub(crate) async fn peer_display_name(&self, peer_id: &Uuid) -> String {
        {
            let map = self.discovered.read().await;
            if let Some(p) = map.values().find(|p| p.peer_id == peer_id.to_string()) {
                return p.peer_name.clone();
            }
        }
        {
            let t = self.trust.read().await;
            if let Some(p) = t.peer_for(peer_id) {
                return p.peer_name.clone();
            }
        }
        short(peer_id)
    }

    pub(crate) async fn register_outgoing_connection(
        &self,
        peer_id: Uuid,
        handle: PeerConnectionHandle,
    ) {
        super::peer_event_loop::spawn_control_plane_loop(
            self.clone(),
            handle.tx.clone(),
            handle.events.subscribe(),
            peer_id,
        );
        let mut events_rx = handle.events.subscribe();
        self.outgoing_connections
            .write()
            .await
            .insert(peer_id, handle);
        let map = self.outgoing_connections.clone();
        tokio::spawn(async move {
            loop {
                match events_rx.recv().await {
                    Ok(PeerEvent::Disconnected { .. }) => {
                        map.write().await.remove(&peer_id);
                        break;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "peer event stream lagged; continuing");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        map.write().await.remove(&peer_id);
                        break;
                    }
                }
            }
        });
    }
}

pub(crate) fn short(u: &Uuid) -> String {
    u.to_string().chars().take(8).collect()
}

pub(crate) fn pick_default_output_device_id() -> Option<String> {
    list_devices()
        .ok()?
        .into_iter()
        .find(|d| d.kind == DeviceKind::Output)
        .map(|d| d.id)
}

#[cfg(test)]
pub(crate) fn test_ctx() -> DaemonContext {
    let dir = tempfile::tempdir().unwrap();
    let identity = PeerIdentity {
        peer_id: Uuid::new_v4(),
        peer_name: "test".into(),
    };
    let local = identity.peer_id;
    DaemonContext {
        identity,
        trust: Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("trust.toml")).unwrap(),
        )),
        sessions: SessionManager::new(),
        stream_registry: StreamRegistry::new(),
        discovered: Arc::default(),
        outgoing_connections: Arc::default(),
        local_peer_id: local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered_peer(peer_id: Uuid, name: &str) -> DiscoveredPeer {
        DiscoveredPeer {
            peer_id: peer_id.to_string(),
            peer_name: name.into(),
            host: "127.0.0.1".into(),
            port: 5000,
            version: "test".into(),
        }
    }

    #[tokio::test]
    async fn peer_display_name_prefers_discovered() {
        let ctx = test_ctx();
        let peer = Uuid::new_v4();
        ctx.discovered
            .write()
            .await
            .insert(peer.to_string(), discovered_peer(peer, "Alice"));
        assert_eq!(ctx.peer_display_name(&peer).await, "Alice");
    }

    #[tokio::test]
    async fn peer_display_name_falls_back_to_short_uuid() {
        let ctx = test_ctx();
        let peer = Uuid::new_v4();
        assert_eq!(ctx.peer_display_name(&peer).await, short(&peer));
    }
}
