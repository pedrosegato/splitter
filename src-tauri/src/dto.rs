use serde::{Deserialize, Serialize};
use specta::Type;
use splitter_core::net::signaling::server::PendingPeer;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IdentityDto {
    pub peer_id: String,
    pub peer_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PendingPeerDto {
    pub peer_id: String,
    pub peer_name: String,
    pub addr: String,
    /// Absent when the peer is already paired and only asks to be let in.
    pub pairing_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PendingStreamRequestDto {
    pub id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub source_is_system: bool,
    pub source_device_name: String,
}

impl From<&PendingPeer> for PendingPeerDto {
    fn from(p: &PendingPeer) -> Self {
        Self {
            peer_id: p.peer_id.to_string(),
            peer_name: p.peer_name.clone(),
            addr: p.remote_addr.to_string(),
            pairing_code: p.pairing_code.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::net::signaling::server::PendingPeer;
    #[test]
    fn pending_peer_dto_maps_fields() {
        let p = PendingPeer {
            peer_id: uuid::Uuid::nil(),
            peer_name: "Studio PC".into(),
            remote_addr: "192.168.0.21:51000".parse().unwrap(),
            connection_id: uuid::Uuid::nil(),
            pairing_code: Some("123 456".into()),
            token: "secret".into(),
        };
        let dto = PendingPeerDto::from(&p);
        assert_eq!(dto.peer_id, uuid::Uuid::nil().to_string());
        assert_eq!(dto.addr, "192.168.0.21:51000");
        assert_eq!(dto.pairing_code.as_deref(), Some("123 456"));
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("secret"), "the token never reaches the UI");
    }
}
