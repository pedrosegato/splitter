use crate::error::NetError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerIdentity {
    pub peer_id: Uuid,
    pub peer_name: String,
}

impl PeerIdentity {
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
            "audiomirror-peer".into()
        } else {
            hostname
        };
        let identity = Self {
            peer_id: Uuid::new_v4(),
            peer_name,
        };
        let serialized = toml::to_string_pretty(&identity)
            .map_err(|e| NetError::ConfigIo(format!("serialize: {e}")))?;
        std::fs::write(path, serialized)
            .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", path.display())))?;
        Ok(identity)
    }
}

pub fn identity_path() -> Result<PathBuf, NetError> {
    let base = dirs::config_dir()
        .ok_or_else(|| NetError::ConfigIo("no config_dir available on this platform".into()))?;
    Ok(base.join("AudioMirror").join("identity.toml"))
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
}
