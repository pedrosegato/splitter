use crate::error::NetError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedPeer {
    pub peer_id: Uuid,
    pub peer_name: String,
    pub auth_token: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStoreFile {
    #[serde(default)]
    pub trusted: HashMap<String, TrustedPeer>,
}

#[derive(Debug)]
pub struct TrustStore {
    path: PathBuf,
    trusted: HashMap<Uuid, TrustedPeer>,
}

impl TrustStore {
    pub fn load_or_create(path: &Path) -> Result<Self, NetError> {
        let trusted = if path.exists() {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| NetError::ConfigIo(format!("read {}: {e}", path.display())))?;
            let parsed: TrustStoreFile = toml::from_str(&raw)
                .map_err(|e| NetError::ConfigIo(format!("parse {}: {e}", path.display())))?;
            parsed
                .trusted
                .into_iter()
                .filter_map(|(k, v)| Uuid::parse_str(&k).ok().map(|id| (id, v)))
                .collect()
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", parent.display())))?;
            }
            HashMap::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            trusted,
        })
    }

    pub fn add(&mut self, peer: TrustedPeer) -> Result<(), NetError> {
        self.trusted.insert(peer.peer_id, peer);
        self.flush()
    }

    pub fn verify(&self, peer_id: &Uuid, token: &str) -> bool {
        self.trusted
            .get(peer_id)
            .map(|p| p.auth_token == token)
            .unwrap_or(false)
    }

    pub fn contains(&self, peer_id: &Uuid) -> bool {
        self.trusted.contains_key(peer_id)
    }

    pub fn token_for(&self, peer_id: Option<Uuid>) -> Option<String> {
        peer_id
            .and_then(|id| self.trusted.get(&id))
            .map(|p| p.auth_token.clone())
    }

    pub fn peer_for(&self, peer_id: &Uuid) -> Option<&TrustedPeer> {
        self.trusted.get(peer_id)
    }

    pub fn first_peer_id(&self) -> Option<Uuid> {
        self.trusted.keys().next().copied()
    }

    fn flush(&self) -> Result<(), NetError> {
        let serializable: HashMap<String, TrustedPeer> = self
            .trusted
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let file = TrustStoreFile {
            trusted: serializable,
        };
        let raw = toml::to_string_pretty(&file)
            .map_err(|e| NetError::ConfigIo(format!("serialize trust: {e}")))?;
        std::fs::write(&self.path, raw)
            .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", self.path.display())))?;
        Ok(())
    }
}

pub fn trust_store_path() -> Result<PathBuf, NetError> {
    let base = dirs::config_dir()
        .ok_or_else(|| NetError::ConfigIo("no config_dir available on this platform".into()))?;
    Ok(base.join("Splitter").join("trusted_peers.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample(peer_id: Uuid, token: &str) -> TrustedPeer {
        TrustedPeer {
            peer_id,
            peer_name: "test".into(),
            auth_token: token.into(),
        }
    }

    #[test]
    fn load_or_create_returns_empty_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let store = TrustStore::load_or_create(&path).expect("create");
        assert!(!store.contains(&Uuid::new_v4()));
    }

    #[test]
    fn add_persists_and_verify_succeeds_on_reload() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let mut store = TrustStore::load_or_create(&path).expect("create");
        let peer_id = Uuid::new_v4();
        store.add(sample(peer_id, "tok-xyz")).unwrap();
        assert!(store.verify(&peer_id, "tok-xyz"));
        assert!(!store.verify(&peer_id, "wrong"));

        let reloaded = TrustStore::load_or_create(&path).expect("reload");
        assert!(reloaded.verify(&peer_id, "tok-xyz"));
    }

    #[test]
    fn verify_unknown_peer_returns_false() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let store = TrustStore::load_or_create(&path).expect("create");
        assert!(!store.verify(&Uuid::new_v4(), "tok"));
    }
}
