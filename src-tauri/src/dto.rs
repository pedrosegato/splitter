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
}

impl From<&PendingPeer> for PendingPeerDto {
    fn from(p: &PendingPeer) -> Self {
        Self {
            peer_id: p.peer_id.to_string(),
            peer_name: p.peer_name.clone(),
            addr: p.remote_addr.to_string(),
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
            proposed_token: "tok".into(),
        };
        let dto = PendingPeerDto::from(&p);
        assert_eq!(dto.peer_id, uuid::Uuid::nil().to_string());
        assert_eq!(dto.addr, "192.168.0.21:51000");
    }
}
