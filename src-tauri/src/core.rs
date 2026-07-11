use crate::events::{DevicesChanged, PeersChanged, StatsTick, StreamStat};
use parking_lot::RwLock as ParkingRwLock;
use splitter_core::net::device_watcher::DeviceEvent;
use splitter_core::net::discovery::{DiscoveredPeer, DiscoveryEvent, SERVICE_TYPE};
use splitter_core::net::signaling::connection::PeerConnectionHandle;
use splitter_core::net::signaling::server::{SignalingServer, SignalingServerHandle};
use splitter_core::net::signaling::{DeviceDescriptor, SignalingMessage};
use splitter_core::settings::SettingsHandle;
use splitter_core::{PeerIdentity, SessionManager, Settings, StreamRegistry, TrustStore};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

pub fn apply_discovery_event(map: &mut HashMap<String, DiscoveredPeer>, ev: DiscoveryEvent) {
    match ev {
        DiscoveryEvent::Found(p) => {
            map.insert(p.peer_id.clone(), p);
        }
        DiscoveryEvent::Removed(fullname) => {
            map.retain(|peer_id, _| format!("{peer_id}.{SERVICE_TYPE}") != fullname);
        }
    }
}

pub fn apply_peer_rename(
    map: &mut HashMap<String, DiscoveredPeer>,
    peer_id: &str,
    peer_name: &str,
) -> bool {
    if let Some(p) = map.get_mut(peer_id) {
        p.peer_name = peer_name.to_string();
        true
    } else {
        false
    }
}

pub struct AppCore {
    pub identity: ParkingRwLock<PeerIdentity>,
    pub settings: SettingsHandle,
    pub trust: Arc<RwLock<TrustStore>>,
    pub sessions: Arc<SessionManager>,
    pub stream_registry: Arc<StreamRegistry>,
    pub server: SignalingServerHandle,
    pub outgoing: Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>,
    pub local_disconnects: Arc<RwLock<HashSet<Uuid>>>,
    pub peers: Arc<RwLock<HashMap<String, DiscoveredPeer>>>,
    pub remote_devices: Arc<RwLock<HashMap<Uuid, Vec<DeviceDescriptor>>>>,
    pub app: OnceLock<tauri::AppHandle>,
    pub discovery: OnceLock<splitter_core::net::discovery::DiscoveryHandle>,
}

impl AppCore {
    pub async fn init(config_dir: &Path) -> Result<Arc<Self>, String> {
        let identity =
            PeerIdentity::load_or_create(&config_dir.join("identity.toml")).map_err(e2s)?;
        let loaded_settings =
            Settings::load_or_default(&config_dir.join("settings.toml")).map_err(e2s)?;
        let signaling_port = loaded_settings.signaling_port;
        let settings: SettingsHandle = Arc::new(RwLock::new(loaded_settings));
        let trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&config_dir.join("trust.toml")).map_err(e2s)?,
        ));
        let sessions = SessionManager::new();
        let stream_registry = StreamRegistry::new();
        let preferred_bind: SocketAddr =
            format!("0.0.0.0:{signaling_port}").parse().map_err(e2s)?;
        let server = match SignalingServer::start(
            preferred_bind,
            identity.clone(),
            trust.clone(),
            sessions.clone(),
            settings.clone(),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("failed to bind signaling server on port {signaling_port}: {e}; retrying on OS-assigned port");
                let fallback_bind: SocketAddr = "0.0.0.0:0".parse().map_err(e2s)?;
                SignalingServer::start(
                    fallback_bind,
                    identity.clone(),
                    trust.clone(),
                    sessions.clone(),
                    settings.clone(),
                )
                .await
                .map_err(e2s)?
            }
        };
        Ok(Arc::new(Self {
            identity: ParkingRwLock::new(identity),
            settings,
            trust,
            sessions,
            stream_registry,
            server,
            outgoing: Arc::new(RwLock::new(HashMap::new())),
            local_disconnects: Arc::new(RwLock::new(HashSet::new())),
            peers: Arc::new(RwLock::new(HashMap::new())),
            remote_devices: Arc::new(RwLock::new(HashMap::new())),
            app: OnceLock::new(),
            discovery: OnceLock::new(),
        }))
    }
}

impl AppCore {
    pub async fn evict_peer_connection(&self, peer_id: &Uuid) {
        self.server.connections.write().await.remove(peer_id);
        self.outgoing.write().await.remove(peer_id);
        self.remote_devices.write().await.remove(peer_id);
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
        let identity = self.identity.read().clone();
        let discovery = splitter_core::net::discovery::Discovery::start(&identity, signaling_port)
            .map_err(e2s)?;
        let _ = self.discovery.set(discovery.handle());
        let core = self.clone();
        let mut discovery = discovery;
        tauri::async_runtime::spawn(async move {
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
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            let mut baseline = splitter_core::net::stream_runtime::StatsBaseline::default();
            loop {
                ticker.tick().await;
                let raw = core.stream_registry.snapshot_stats(1000, &mut baseline).await;
                let stats: Vec<StreamStat> = raw
                    .into_iter()
                    .map(|(session_id, stream_id, snap)| StreamStat {
                        session_id: session_id.to_string(),
                        stream_id: stream_id.get(),
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
    pub fn spawn_device_watcher(self: &Arc<Self>) {
        let core = self.clone();
        tauri::async_runtime::spawn(async move {
            let watcher =
                splitter_core::net::device_watcher::start(Duration::from_secs(2));
            let mut rx = watcher.subscribe();
            while let Ok(ev) = rx.recv().await {
                match ev {
                    DeviceEvent::Appeared(_) | DeviceEvent::Disappeared(_) => {
                        core.on_devices_changed().await;
                    }
                }
            }
        });
    }

    pub async fn on_devices_changed(&self) {
        let devices = local_device_descriptors();
        self.send_to_all_peers(SignalingMessage::DeviceListResponse { devices })
            .await;
        self.emit(DevicesChanged);
    }

    async fn send_to_all_peers(&self, msg: SignalingMessage) {
        for handle in self.server.connections.read().await.values() {
            let _ = handle.tx.send(msg.clone()).await;
        }
        for handle in self.outgoing.read().await.values() {
            let _ = handle.tx.send(msg.clone()).await;
        }
    }
}

fn local_device_descriptors() -> Vec<DeviceDescriptor> {
    splitter_core::audio::devices::list_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| DeviceDescriptor {
            id: d.id,
            name: d.name,
            kind: d.kind,
        })
        .collect()
}

impl AppCore {
    pub fn spawn_acceptor_supervisor(self: &Arc<Self>) {
        let host = Arc::new(crate::acceptor::TauriControlPlane { core: self.clone() });
        let established = self.server.connection_established_tx.subscribe();
        let connections = self.server.connections.clone();
        tauri::async_runtime::spawn(async move {
            splitter_core::net::signaling::spawn_connection_supervisor(established, connections, host);
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
    async fn identity_is_readable_through_lock() {
        let dir = tempdir().unwrap();
        let core = AppCore::init(dir.path()).await.expect("init");
        let name = core.identity.read().peer_name.clone();
        assert!(!name.is_empty());
    }

    #[test]
    fn identity_lock_not_poisoned_by_panic() {
        use parking_lot::RwLock as ParkingRwLock;
        use splitter_core::PeerIdentity;
        use uuid::Uuid;
        let lock = ParkingRwLock::new(PeerIdentity {
            peer_id: Uuid::new_v4(),
            peer_name: "test".into(),
        });
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = lock.write();
            panic!("boom");
        }));
        let _g = lock.read();
    }

    #[test]
    fn apply_peer_rename_updates_existing_entry() {
        use splitter_core::net::discovery::DiscoveredPeer;
        let mut map = std::collections::HashMap::new();
        map.insert(
            "id1".to_string(),
            DiscoveredPeer {
                peer_id: "id1".into(),
                peer_name: "Old".into(),
                host: "10.0.0.2".into(),
                port: 7000,
                version: "0.1.0".into(),
            },
        );
        let changed = apply_peer_rename(&mut map, "id1", "New");
        assert!(changed);
        assert_eq!(map.get("id1").unwrap().peer_name, "New");
        assert!(!apply_peer_rename(&mut map, "missing", "X"));
    }

    async fn loopback_handle() -> PeerConnectionHandle {
        use splitter_core::net::signaling::connection::spawn_peer_connection;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let laddr = listener.local_addr().unwrap();
        let (client, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(laddr), listener.accept());
        accepted.unwrap();
        spawn_peer_connection(client.unwrap(), None).unwrap()
    }

    #[tokio::test]
    async fn evict_peer_connection_removes_from_all_connection_maps() {
        let dir = tempdir().unwrap();
        let core = AppCore::init(dir.path()).await.expect("init");
        let peer_id = Uuid::new_v4();

        core.server
            .connections
            .write()
            .await
            .insert(peer_id, loopback_handle().await);
        core.outgoing
            .write()
            .await
            .insert(peer_id, loopback_handle().await);
        core.remote_devices
            .write()
            .await
            .insert(peer_id, Vec::new());
        core.peers.write().await.insert(
            "mdns-peer".to_string(),
            DiscoveredPeer {
                peer_id: "mdns-peer".into(),
                peer_name: "keep".into(),
                host: "10.0.0.9".into(),
                port: 7000,
                version: "0.1.0".into(),
            },
        );

        core.evict_peer_connection(&peer_id).await;

        assert!(!core.server.connections.read().await.contains_key(&peer_id));
        assert!(!core.outgoing.read().await.contains_key(&peer_id));
        assert!(!core.remote_devices.read().await.contains_key(&peer_id));
        assert!(
            core.peers.read().await.contains_key("mdns-peer"),
            "mDNS discovery map must not be evicted on connection drop"
        );
    }

    #[tokio::test]
    async fn device_change_sends_device_list_to_connected_peer() {
        use splitter_core::net::signaling::connection::spawn_peer_connection;
        use splitter_core::net::signaling::{PeerEvent, SignalingMessage};
        let dir = tempdir().unwrap();
        let core = AppCore::init(dir.path()).await.expect("init");
        let peer_id = Uuid::new_v4();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let laddr = listener.local_addr().unwrap();
        let (client, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(laddr), listener.accept());
        let (server_stream, _) = accepted.unwrap();
        let client_handle = spawn_peer_connection(client.unwrap(), None).unwrap();
        let server_handle = spawn_peer_connection(server_stream, None).unwrap();
        let mut server_events = server_handle.events.subscribe();

        core.outgoing.write().await.insert(peer_id, client_handle);
        core.on_devices_changed().await;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(PeerEvent::Message(SignalingMessage::DeviceListResponse { .. })) =
                    server_events.recv().await
                {
                    return;
                }
            }
        })
        .await
        .expect("no DeviceListResponse delivered to connected peer within 2s");
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
        apply_discovery_event(
            &mut map,
            DiscoveryEvent::Removed("id1._splitter._tcp.local.".into()),
        );
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn removal_empty_fullname_does_not_remove_valid_peers() {
        use splitter_core::net::discovery::{DiscoveredPeer, DiscoveryEvent};
        let mut map = std::collections::HashMap::new();
        map.insert(
            "id1".to_string(),
            DiscoveredPeer {
                peer_id: "id1".into(),
                peer_name: "Peer".into(),
                host: "10.0.0.1".into(),
                port: 7000,
                version: "0.1.0".into(),
            },
        );
        apply_discovery_event(&mut map, DiscoveryEvent::Removed("".into()));
        assert_eq!(
            map.len(),
            1,
            "empty fullname must not remove existing peers"
        );
    }

    #[test]
    fn removal_keys_on_exact_fullname_not_substring() {
        use splitter_core::net::discovery::{DiscoveredPeer, DiscoveryEvent};
        let mut map = std::collections::HashMap::new();
        map.insert(
            "abc".to_string(),
            DiscoveredPeer {
                peer_id: "abc".into(),
                peer_name: "Keep".into(),
                host: "10.0.0.2".into(),
                port: 7001,
                version: "0.1.0".into(),
            },
        );
        map.insert(
            "abcdef".to_string(),
            DiscoveredPeer {
                peer_id: "abcdef".into(),
                peer_name: "Remove".into(),
                host: "10.0.0.3".into(),
                port: 7002,
                version: "0.1.0".into(),
            },
        );
        apply_discovery_event(
            &mut map,
            DiscoveryEvent::Removed("abcdef._splitter._tcp.local.".into()),
        );
        assert_eq!(map.len(), 1, "only the exact peer should be removed");
        assert!(map.contains_key("abc"), "non-target peer must survive");
    }
}
