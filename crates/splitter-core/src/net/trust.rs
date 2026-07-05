use crate::error::NetError;
use crate::net::fs_util::write_atomic;
use constant_time_eq::constant_time_eq;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const MIN_AUTH_TOKEN_LEN: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedPeer {
    pub peer_id: Uuid,
    pub peer_name: String,
    pub auth_token: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStoreFile {
    #[serde(default)]
    pub trusted: Vec<TrustedPeer>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LegacyTrustStoreFile {
    #[serde(default)]
    trusted: HashMap<String, LegacyTrustedPeer>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyTrustedPeer {
    peer_name: String,
    auth_token: String,
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
            let (map, needs_flush) = match toml::from_str::<TrustStoreFile>(&raw) {
                Ok(parsed) => {
                    let mut map = HashMap::new();
                    for entry in parsed.trusted {
                        if map.contains_key(&entry.peer_id) {
                            tracing::warn!(
                                peer_id = %entry.peer_id,
                                "trust store: duplicate peer_id entry; keeping first"
                            );
                            continue;
                        }
                        map.insert(entry.peer_id, entry);
                    }
                    (map, false)
                }
                Err(_) => match toml::from_str::<LegacyTrustStoreFile>(&raw) {
                    Ok(legacy) => {
                        tracing::warn!(
                            path = %path.display(),
                            "trust store: migrating legacy HashMap format to Vec format"
                        );
                        let mut map = HashMap::new();
                        for (key, entry) in legacy.trusted {
                            match key.parse::<Uuid>() {
                                Ok(id) => {
                                    map.insert(
                                        id,
                                        TrustedPeer {
                                            peer_id: id,
                                            peer_name: entry.peer_name,
                                            auth_token: entry.auth_token,
                                        },
                                    );
                                }
                                Err(_) => {
                                    tracing::warn!(
                                        key = %key,
                                        "trust store: legacy key is not a valid UUID; skipping"
                                    );
                                }
                            }
                        }
                        (map, true)
                    }
                    Err(e) => {
                        return Err(NetError::ConfigIo(format!("parse {}: {e}", path.display())));
                    }
                },
            };
            if needs_flush {
                let store = Self {
                    path: path.to_path_buf(),
                    trusted: map,
                };
                store.flush()?;
                return Ok(store);
            }
            map
        } else {
            if let Some(parent) = path.parent() {
                crate::net::fs_util::ensure_private_dir(parent)
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
        if peer.auth_token.len() < MIN_AUTH_TOKEN_LEN {
            return Err(NetError::SignalingProtocol {
                reason: "auth token below minimum entropy length".into(),
            });
        }
        self.trusted.insert(peer.peer_id, peer);
        self.flush()
    }

    pub fn verify(&self, peer_id: &Uuid, token: &str) -> bool {
        self.trusted
            .get(peer_id)
            .map(|p| constant_time_eq(p.auth_token.as_bytes(), token.as_bytes()))
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
        let file = TrustStoreFile {
            trusted: self.trusted.values().cloned().collect(),
        };
        let raw = toml::to_string_pretty(&file)
            .map_err(|e| NetError::ConfigIo(format!("serialize trust: {e}")))?;
        write_atomic(&self.path, raw.as_bytes())
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

    fn valid_token() -> String {
        "a".repeat(MIN_AUTH_TOKEN_LEN + 11)
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
        let token = valid_token();
        store.add(sample(peer_id, &token)).unwrap();
        assert!(store.verify(&peer_id, &token));
        assert!(!store.verify(&peer_id, "wrong"));

        let reloaded = TrustStore::load_or_create(&path).expect("reload");
        assert!(reloaded.verify(&peer_id, &token));
    }

    #[test]
    fn add_leaves_no_tmp_file_and_content_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let mut store = TrustStore::load_or_create(&path).expect("create");
        let peer_id = Uuid::new_v4();
        let token = valid_token();
        store.add(sample(peer_id, &token)).unwrap();
        let tmp = path.with_extension("toml.tmp");
        assert!(!tmp.exists(), "no tmp file after flush");
        let reloaded = TrustStore::load_or_create(&path).expect("reload");
        assert!(reloaded.verify(&peer_id, &token));
    }

    #[test]
    fn verify_unknown_peer_returns_false() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let store = TrustStore::load_or_create(&path).expect("create");
        assert!(!store.verify(&Uuid::new_v4(), "tok"));
    }

    #[test]
    fn legacy_hashmap_format_migrates_and_rewrites_as_vec() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let peer_id = Uuid::new_v4();
        let legacy = format!(
            "[trusted.{}]\npeer_name = \"Alice\"\nauth_token = \"tok-legacy\"\n",
            peer_id
        );
        std::fs::write(&path, legacy).unwrap();

        let store = TrustStore::load_or_create(&path).expect("migrate");
        assert!(
            store.verify(&peer_id, "tok-legacy"),
            "migrated peer must be verifiable"
        );

        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(
            rewritten.contains("[[trusted]]"),
            "file must be rewritten in Vec format: {rewritten}"
        );

        let reloaded = TrustStore::load_or_create(&path).expect("reload after migration");
        assert!(reloaded.verify(&peer_id, "tok-legacy"));
    }

    #[test]
    fn legacy_format_with_invalid_uuid_key_warns_and_skips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let legacy =
            "[trusted.not-a-uuid]\npeer_name = \"Bob\"\nauth_token = \"tok-bad\"\n".to_string();
        std::fs::write(&path, legacy).unwrap();

        let store = TrustStore::load_or_create(&path).expect("migrate with bad key");
        assert!(store.trusted.is_empty(), "bad-uuid entry must be skipped");
    }

    #[test]
    fn add_rejects_below_min_length() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted_peers.toml");
        let mut store = TrustStore::load_or_create(&path).expect("create");
        let peer_id = Uuid::new_v4();

        assert!(
            store.add(sample(peer_id, "short")).is_err(),
            "token below MIN_AUTH_TOKEN_LEN must be rejected"
        );
        assert!(!store.contains(&peer_id));

        let token = valid_token();
        assert!(
            store.add(sample(peer_id, &token)).is_ok(),
            "token at or above MIN_AUTH_TOKEN_LEN must be accepted"
        );
        assert!(store.verify(&peer_id, &token));
    }
}
