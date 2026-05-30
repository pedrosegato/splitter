use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;
use splitter_core::{PeerIdentity, SessionManager, Settings, StreamRegistry, TrustStore};
use splitter_core::settings::SettingsHandle;
use splitter_core::net::signaling::server::{SignalingServer, SignalingServerHandle};
use splitter_core::net::signaling::connection::PeerConnectionHandle;
use splitter_core::net::discovery::{DiscoveredPeer, DiscoveryEvent};

pub fn apply_discovery_event(map: &mut HashMap<String, DiscoveredPeer>, ev: DiscoveryEvent) {
    match ev {
        DiscoveryEvent::Found(p) => {
            map.insert(p.peer_id.clone(), p);
        }
        DiscoveryEvent::Removed(fullname) => {
            map.retain(|peer_id, _| !fullname.contains(peer_id.as_str()));
        }
    }
}

pub struct AppCore {
    pub identity: PeerIdentity,
    pub settings: SettingsHandle,
    pub trust: Arc<RwLock<TrustStore>>,
    pub sessions: Arc<SessionManager>,
    pub stream_registry: Arc<StreamRegistry>,
    pub server: SignalingServerHandle,
    pub outgoing: Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
    pub peers: Arc<RwLock<HashMap<String, DiscoveredPeer>>>,
}

impl AppCore {
    pub async fn init(config_dir: &Path, signaling_port: u16) -> Result<Arc<Self>, String> {
        let identity = PeerIdentity::load_or_create(&config_dir.join("identity.toml")).map_err(e2s)?;
        let settings: SettingsHandle = Arc::new(RwLock::new(
            Settings::load_or_default(&config_dir.join("settings.toml")).map_err(e2s)?,
        ));
        let trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&config_dir.join("trust.toml")).map_err(e2s)?,
        ));
        let sessions = SessionManager::new();
        let stream_registry = StreamRegistry::new();
        let bind: SocketAddr = format!("0.0.0.0:{signaling_port}").parse().map_err(e2s)?;
        let server = SignalingServer::start(bind, identity.clone(), trust.clone(), sessions.clone(), settings.clone())
            .await
            .map_err(e2s)?;
        Ok(Arc::new(Self {
            identity,
            settings,
            trust,
            sessions,
            stream_registry,
            server,
            outgoing: Arc::new(RwLock::new(HashMap::new())),
            peers: Arc::new(RwLock::new(HashMap::new())),
        }))
    }
}

impl AppCore {
    pub fn spawn_discovery(self: &Arc<Self>, signaling_port: u16) -> Result<(), String> {
        let mut discovery = splitter_core::net::discovery::Discovery::start(&self.identity, signaling_port)
            .map_err(e2s)?;
        let peers = self.peers.clone();
        tokio::spawn(async move {
            while let Some(ev) = discovery.next_event().await {
                apply_discovery_event(&mut *peers.write().await, ev);
            }
        });
        Ok(())
    }
}

impl AppCore {
    pub fn spawn_acceptor_supervisor(self: &Arc<Self>) {
        let core = self.clone();
        let mut established = self.server.connection_established_tx.subscribe();
        tokio::spawn(async move {
            loop {
                match established.recv().await {
                    Ok(peer_id) => {
                        let events = {
                            let conns = core.server.connections.read().await;
                            conns.get(&peer_id).map(|c| c.events.subscribe())
                        };
                        if let Some(events) = events {
                            crate::acceptor::spawn_acceptor(core.clone(), peer_id, events);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn init_builds_all_handles_in_temp_dir() {
        let dir = tempdir().unwrap();
        let core = AppCore::init(dir.path(), 0).await.expect("init");
        assert!(core.server.bind_addr.port() > 0);
        assert_eq!(core.sessions.snapshot().await.len(), 0);
    }

    #[tokio::test]
    async fn discovery_reducer_adds_and_removes() {
        use splitter_core::net::discovery::{DiscoveredPeer, DiscoveryEvent};
        let mut map = std::collections::HashMap::new();
        let p = DiscoveredPeer {
            peer_id: "id1".into(),
            peer_name: "Studio PC".into(),
            host: "192.168.0.21".into(),
            port: 7000,
            version: "0.1.0".into(),
        };
        apply_discovery_event(&mut map, DiscoveryEvent::Found(p.clone()));
        assert_eq!(map.len(), 1);
        apply_discovery_event(&mut map, DiscoveryEvent::Removed("id1._splitter._tcp.local.".into()));
        assert_eq!(map.len(), 0);
    }
}
