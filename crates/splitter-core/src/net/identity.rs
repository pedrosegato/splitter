use crate::error::NetError;
use crate::net::fs_util::write_atomic;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerIdentity {
    pub peer_id: Uuid,
    pub peer_name: String,
}

impl PeerIdentity {
    pub fn save_atomic(&self, path: &Path) -> Result<(), NetError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", parent.display())))?;
        }
        let raw = toml::to_string_pretty(self)
            .map_err(|e| NetError::ConfigIo(format!("serialize identity: {e}")))?;
        write_atomic(path, raw.as_bytes())
            .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", path.display())))
    }

    pub fn load_or_create(path: &Path) -> Result<Self, NetError> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| NetError::ConfigIo(format!("read {}: {e}", path.display())))?;
            let parsed: Self = toml::from_str(&raw)
                .map_err(|e| NetError::ConfigIo(format!("parse {}: {e}", path.display())))?;
            return Ok(parsed);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", parent.display())))?;
        }
        let hostname = gethostname::gethostname().to_string_lossy().to_string();
        let peer_name = if hostname.is_empty() {
            "splitter-peer".into()
        } else {
            hostname
        };
        let identity = Self {
            peer_id: Uuid::new_v4(),
            peer_name,
        };
        let serialized = toml::to_string_pretty(&identity)
            .map_err(|e| NetError::ConfigIo(format!("serialize: {e}")))?;
        write_atomic(path, serialized.as_bytes())
            .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", path.display())))?;
        Ok(identity)
    }
}

pub fn identity_path() -> Result<PathBuf, NetError> {
    let base = dirs::config_dir()
        .ok_or_else(|| NetError::ConfigIo("no config_dir available on this platform".into()))?;
    Ok(base.join("Splitter").join("identity.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_or_create_creates_file_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.toml");
        let id = PeerIdentity::load_or_create(&path).expect("create");
        assert!(path.exists());
        assert!(!id.peer_name.is_empty());
    }

    #[test]
    fn load_or_create_returns_existing_on_second_call() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.toml");
        let first = PeerIdentity::load_or_create(&path).expect("create");
        let second = PeerIdentity::load_or_create(&path).expect("reload");
        assert_eq!(first, second);
    }

    #[test]
    fn load_or_create_writes_valid_toml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.toml");
        let _ = PeerIdentity::load_or_create(&path).expect("create");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("peer_id"));
        assert!(raw.contains("peer_name"));
    }

    #[test]
    fn different_paths_produce_different_peer_ids() {
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();
        let path_a = dir_a.path().join("identity.toml");
        let path_b = dir_b.path().join("identity.toml");
        let id_a = PeerIdentity::load_or_create(&path_a).expect("create a");
        let id_b = PeerIdentity::load_or_create(&path_b).expect("create b");
        assert_ne!(id_a.peer_id, id_b.peer_id);
    }

    #[test]
    fn load_or_create_leaves_no_tmp_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.toml");
        let id = PeerIdentity::load_or_create(&path).expect("create");
        let tmp = path.with_extension("toml.tmp");
        assert!(!tmp.exists(), "no tmp file after create");
        let reloaded = PeerIdentity::load_or_create(&path).expect("reload");
        assert_eq!(id, reloaded);
    }

    #[test]
    fn save_atomic_persists_updated_name() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.toml");
        let mut id = PeerIdentity::load_or_create(&path).expect("create");
        id.peer_name = "Estúdio do Pedro".into();
        id.save_atomic(&path).expect("save");
        let reloaded = PeerIdentity::load_or_create(&path).expect("reload");
        assert_eq!(reloaded.peer_name, "Estúdio do Pedro");
        assert_eq!(reloaded.peer_id, id.peer_id);
    }
}
