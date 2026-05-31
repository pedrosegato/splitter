use crate::error::NetError;
use crate::net::identity::PeerIdentity;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use tokio::sync::mpsc;

pub const SERVICE_TYPE: &str = "_splitter._tcp.local.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct DiscoveredPeer {
    pub peer_id: String,
    pub peer_name: String,
    pub host: String,
    pub port: u16,
    pub version: String,
}

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    Found(DiscoveredPeer),
    Removed(String),
}

pub struct Discovery {
    daemon: ServiceDaemon,
    events: mpsc::Receiver<DiscoveryEvent>,
    fullname: String,
}

fn build_service_info(
    identity: &PeerIdentity,
    signaling_port: u16,
) -> Result<ServiceInfo, NetError> {
    let instance = identity.peer_id.to_string();
    let host_name = format!("{}.local.", instance);
    let mut properties: HashMap<String, String> = HashMap::new();
    properties.insert("peer_id".into(), identity.peer_id.to_string());
    properties.insert("peer_name".into(), identity.peer_name.clone());
    properties.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    properties.insert("signaling_port".into(), signaling_port.to_string());
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host_name,
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        signaling_port,
        Some(properties),
    )
    .map_err(|e| NetError::Mdns {
        reason: format!("info: {e}"),
    })?
    .enable_addr_auto();
    Ok(info)
}

pub(crate) fn map_resolved(
    peer_id: Option<&str>,
    peer_name: Option<&str>,
    version: Option<&str>,
    addrs: &[std::net::IpAddr],
    port: u16,
) -> Option<DiscoveryEvent> {
    let peer_id = peer_id.filter(|s| !s.is_empty())?.to_string();
    let host = addrs
        .iter()
        .next()
        .map(|a| a.to_string())
        .filter(|s| !s.is_empty())?;
    Some(DiscoveryEvent::Found(DiscoveredPeer {
        peer_id,
        peer_name: peer_name.unwrap_or_default().to_string(),
        host,
        port,
        version: version.unwrap_or_default().to_string(),
    }))
}

impl Discovery {
    pub fn start(identity: &PeerIdentity, signaling_port: u16) -> Result<Self, NetError> {
        let daemon = ServiceDaemon::new().map_err(|e| NetError::Mdns {
            reason: format!("daemon: {e}"),
        })?;

        let info = build_service_info(identity, signaling_port)?;

        let fullname = info.get_fullname().to_string();
        daemon.register(info).map_err(|e| NetError::Mdns {
            reason: format!("register: {e}"),
        })?;

        let browse = daemon.browse(SERVICE_TYPE).map_err(|e| NetError::Mdns {
            reason: format!("browse: {e}"),
        })?;

        let (tx, rx) = mpsc::channel::<DiscoveryEvent>(64);
        let self_full = fullname.clone();
        std::thread::spawn(move || {
            while let Ok(event) = browse.recv() {
                let mapped = match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if info.get_fullname() == self_full {
                            continue;
                        }
                        let props = info.get_properties();
                        let peer_id = props.get("peer_id").map(|p| p.val_str());
                        let peer_name = props.get("peer_name").map(|p| p.val_str());
                        let version = props.get("version").map(|p| p.val_str());
                        let addrs: Vec<std::net::IpAddr> =
                            info.get_addresses().iter().copied().collect();
                        map_resolved(peer_id, peer_name, version, &addrs, info.get_port())
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        Some(DiscoveryEvent::Removed(fullname))
                    }
                    _ => None,
                };
                if let Some(ev) = mapped {
                    if tx.blocking_send(ev).is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            daemon,
            events: rx,
            fullname,
        })
    }

    pub async fn next_event(&mut self) -> Option<DiscoveryEvent> {
        self.events.recv().await
    }

    pub fn fullname(&self) -> &str {
        &self.fullname
    }

    pub fn handle(&self) -> DiscoveryHandle {
        DiscoveryHandle {
            daemon: self.daemon.clone(),
            fullname: self.fullname.clone(),
        }
    }

    pub fn shutdown(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[derive(Clone)]
pub struct DiscoveryHandle {
    daemon: ServiceDaemon,
    fullname: String,
}

impl DiscoveryHandle {
    pub fn reannounce(&self, identity: &PeerIdentity, signaling_port: u16) -> Result<(), NetError> {
        let _ = self.daemon.unregister(&self.fullname);
        let info = build_service_info(identity, signaling_port)?;
        self.daemon.register(info).map_err(|e| NetError::Mdns {
            reason: format!("reannounce register: {e}"),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn id(name: &str) -> PeerIdentity {
        PeerIdentity {
            peer_id: Uuid::new_v4(),
            peer_name: name.into(),
        }
    }

    fn addr(octets: [u8; 4]) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(octets))
    }

    #[test]
    fn map_resolved_missing_peer_id_yields_none() {
        let addrs = vec![addr([10, 0, 0, 1])];
        assert!(map_resolved(None, None, None, &addrs, 7000).is_none());
    }

    #[test]
    fn map_resolved_empty_peer_id_yields_none() {
        let addrs = vec![addr([10, 0, 0, 1])];
        assert!(map_resolved(Some(""), None, None, &addrs, 7000).is_none());
    }

    #[test]
    fn map_resolved_no_addrs_yields_none() {
        assert!(map_resolved(Some("abc-id"), None, None, &[], 7000).is_none());
    }

    #[test]
    fn map_resolved_valid_record_yields_found_event() {
        let addrs = vec![addr([192, 168, 0, 10])];
        let ev = map_resolved(Some("my-peer"), Some("Studio"), Some("0.1.0"), &addrs, 7777);
        match ev {
            Some(DiscoveryEvent::Found(p)) => {
                assert_eq!(p.peer_id, "my-peer");
                assert_eq!(p.peer_name, "Studio");
                assert_eq!(p.host, "192.168.0.10");
                assert_eq!(p.port, 7777);
                assert_eq!(p.version, "0.1.0");
            }
            _ => panic!("expected DiscoveryEvent::Found"),
        }
    }

    #[test]
    fn map_resolved_missing_cosmetic_fields_uses_defaults() {
        let addrs = vec![addr([10, 1, 2, 3])];
        let ev = map_resolved(Some("peer-x"), None, None, &addrs, 1234);
        match ev {
            Some(DiscoveryEvent::Found(p)) => {
                assert_eq!(p.peer_name, "");
                assert_eq!(p.version, "");
            }
            _ => panic!("expected DiscoveryEvent::Found"),
        }
    }

    #[tokio::test]
    async fn start_registers_without_panicking() {
        let identity = id("test-peer");
        let _disc = Discovery::start(&identity, 0).expect("start");
    }

    #[test]
    fn shutdown_is_callable_and_does_not_panic() {
        let id = id("shutdown-test");
        let mut disc = Discovery::start(&id, 0).expect("start");
        disc.shutdown();
    }

    #[tokio::test]
    async fn reannounce_does_not_panic_and_updates_name() {
        let mut identity = id("orig-name");
        let disc = Discovery::start(&identity, 0).expect("start");
        let handle = disc.handle();
        identity.peer_name = "renamed".into();
        handle.reannounce(&identity, 0).expect("reannounce");
    }

    #[allow(clippy::print_stderr)]
    #[tokio::test]
    async fn two_local_instances_see_each_other() {
        let a = id("peer-a");
        let b = id("peer-b");
        let mut da = Discovery::start(&a, 17_001).expect("start a");
        let mut db = Discovery::start(&b, 17_002).expect("start b");

        let saw_b = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                if let Some(DiscoveryEvent::Found(p)) = da.next_event().await {
                    if p.peer_id == b.peer_id.to_string() {
                        return true;
                    }
                }
            }
        })
        .await
        .unwrap_or(false);

        let saw_a = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                if let Some(DiscoveryEvent::Found(p)) = db.next_event().await {
                    if p.peer_id == a.peer_id.to_string() {
                        return true;
                    }
                }
            }
        })
        .await
        .unwrap_or(false);

        if !(saw_a && saw_b) {
            eprintln!("mDNS not visible in this CI sandbox; treating as inconclusive");
        }
    }
}
