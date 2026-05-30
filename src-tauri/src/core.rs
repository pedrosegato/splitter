use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;
use splitter_core::{PeerIdentity, SessionManager, Settings, StreamRegistry, TrustStore};
use splitter_core::settings::SettingsHandle;
use splitter_core::net::signaling::server::{SignalingServer, SignalingServerHandle};
use splitter_core::net::signaling::connection::PeerConnectionHandle;
use splitter_core::net::discovery::{DiscoveredPeer, DiscoveryEvent};
use crate::events::{PeersChanged, StatsTick, StreamStat};

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
    pub app: OnceLock<tauri::AppHandle>,
}

impl AppCore {
    pub async fn init(config_dir: &Path) -> Result<Arc<Self>, String> {
        let identity = PeerIdentity::load_or_create(&config_dir.join("identity.toml")).map_err(e2s)?;
        let loaded_settings = Settings::load_or_default(&config_dir.join("settings.toml")).map_err(e2s)?;
        let signaling_port = loaded_settings.signaling_port;
        let settings: SettingsHandle = Arc::new(RwLock::new(loaded_settings));
        let trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&config_dir.join("trust.toml")).map_err(e2s)?,
        ));
        let sessions = SessionManager::new();
        let stream_registry = StreamRegistry::new();
        let preferred_bind: SocketAddr = format!("0.0.0.0:{signaling_port}").parse().map_err(e2s)?;
        let server = match SignalingServer::start(preferred_bind, identity.clone(), trust.clone(), sessions.clone(), settings.clone()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("failed to bind signaling server on port {signaling_port}: {e}; retrying on OS-assigned port");
                let fallback_bind: SocketAddr = "0.0.0.0:0".parse().map_err(e2s)?;
                SignalingServer::start(fallback_bind, identity.clone(), trust.clone(), sessions.clone(), settings.clone())
                    .await
                    .map_err(e2s)?
            }
        };
        Ok(Arc::new(Self {
            identity,
            settings,
            trust,
            sessions,
            stream_registry,
            server,
            outgoing: Arc::new(RwLock::new(HashMap::new())),
            peers: Arc::new(RwLock::new(HashMap::new())),
            app: OnceLock::new(),
        }))
    }
}

impl AppCore {
    pub fn emit<E>(&self, ev: E)
    where
        E: tauri_specta::Event + serde::Serialize + Clone,
    {
        if let Some(app) = self.app.get() {
            let _ = ev.emit(app);
        }
    }
}

impl AppCore {
    pub fn spawn_discovery(self: &Arc<Self>) -> Result<(), String> {
        let signaling_port = self.server.bind_addr.port();
        let mut discovery = splitter_core::net::discovery::Discovery::start(&self.identity, signaling_port)
            .map_err(e2s)?;
        let core = self.clone();
        tokio::spawn(async move {
            while let Some(ev) = discovery.next_event().await {
                apply_discovery_event(&mut *core.peers.write().await, ev);
                let snapshot: Vec<DiscoveredPeer> =
                    core.peers.read().await.values().cloned().collect();
                core.emit(PeersChanged(snapshot));
            }
        });
        Ok(())
    }
}

impl AppCore {
    pub fn spawn_stats_emitter(self: &Arc<Self>) {
        let core = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let raw = core.stream_registry.snapshot_stats(1000).await;
                let stats: Vec<StreamStat> = raw
                    .into_iter()
                    .map(|(session_id, stream_id, snap)| StreamStat {
                        session_id: session_id.to_string(),
                        stream_id,
                        rtt_ms: snap.last_rtt_ms,
                        loss_pct: loss_pct(snap.packets_received, snap.packets_lost),
                        kbps_sent: snap.bitrate_kbps_sent,
                        kbps_received: snap.bitrate_kbps_received,
                    })
                    .collect();
                core.emit(StatsTick(stats));
            }
        });
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
                        let conn_info = {
                            let conns = core.server.connections.read().await;
                            conns.get(&peer_id).map(|c| (c.events.subscribe(), c.remote_addr))
                        };
                        if let Some((events, addr)) = conn_info {
                            crate::acceptor::spawn_acceptor(core.clone(), peer_id, events, addr);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

fn loss_pct(packets_received: u64, packets_lost: u64) -> f32 {
    packets_lost as f32 / (packets_received + packets_lost).max(1) as f32 * 100.0
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
        let core = AppCore::init(dir.path()).await.expect("init");
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
