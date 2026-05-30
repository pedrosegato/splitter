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
use splitter_core::net::discovery::DiscoveredPeer;

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
}
