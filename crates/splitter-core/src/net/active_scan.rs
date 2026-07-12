use crate::net::discovery::DiscoveredPeer;
use crate::net::signaling::connection::{spawn_peer_connection, PeerEvent};
use crate::net::signaling::message::SignalingMessage;
use ipnet::Ipv4Net;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::TcpStream;

const MIN_PREFIX_LEN: u8 = 22;

#[derive(Debug, Clone)]
pub struct IfaceV4 {
    pub name: String,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
}

fn iface_is_scannable(iface: &IfaceV4) -> bool {
    if iface.ip.is_loopback() || iface.ip.is_link_local() || iface.ip.is_unspecified() {
        return false;
    }
    if iface.name.starts_with("awdl") || iface.name.starts_with("llw") {
        return false;
    }
    true
}

pub fn scan_targets(ifaces: &[IfaceV4]) -> Vec<Ipv4Addr> {
    let own_ips: std::collections::HashSet<Ipv4Addr> = ifaces.iter().map(|i| i.ip).collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for iface in ifaces.iter().filter(|i| iface_is_scannable(i)) {
        let Ok(net) = Ipv4Net::with_netmask(iface.ip, iface.netmask) else {
            continue;
        };
        if net.prefix_len() < MIN_PREFIX_LEN {
            continue;
        }
        for host in net.hosts() {
            if own_ips.contains(&host) {
                continue;
            }
            if seen.insert(host) {
                out.push(host);
            }
        }
    }
    out
}

fn interfaces_v4() -> Vec<IfaceV4> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifaces
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) => Some(IfaceV4 {
                name: i.name,
                ip: v4.ip,
                netmask: v4.netmask,
            }),
            _ => None,
        })
        .collect()
}

pub async fn probe_host(addr: SocketAddr, timeout: Duration) -> Option<DiscoveredPeer> {
    tokio::time::timeout(timeout, async move {
        let stream = TcpStream::connect(addr).await.ok()?;
        let handle = spawn_peer_connection(stream, None).ok()?;
        let mut events = handle.events.subscribe();
        handle.tx.send(SignalingMessage::Probe {}).await.ok()?;
        loop {
            match events.recv().await {
                Ok(PeerEvent::Message(SignalingMessage::ProbeAck {
                    peer_id,
                    peer_name,
                    app_version,
                })) => {
                    if peer_id.is_empty() {
                        return None;
                    }
                    return Some(DiscoveredPeer {
                        peer_id,
                        peer_name,
                        host: addr.ip().to_string(),
                        port: addr.port(),
                        version: app_version,
                    });
                }
                Ok(PeerEvent::Disconnected { .. }) | Err(_) => return None,
                Ok(_) => continue,
            }
        }
    })
    .await
    .ok()
    .flatten()
}

pub async fn scan_once(
    local_peer_id: &str,
    port: u16,
    per_host_timeout: Duration,
    concurrency: usize,
) -> Vec<DiscoveredPeer> {
    use futures::stream::StreamExt;

    let targets = scan_targets(&interfaces_v4());
    let local = local_peer_id.to_string();

    futures::stream::iter(targets.into_iter().map(|ip| {
        let addr = SocketAddr::new(IpAddr::V4(ip), port);
        async move { probe_host(addr, per_host_timeout).await }
    }))
    .buffer_unordered(concurrency.max(1))
    .filter_map(|found| async move { found })
    .filter(|peer| {
        let is_self = peer.peer_id == local;
        async move { !is_self }
    })
    .collect()
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, ip: [u8; 4], mask: [u8; 4]) -> IfaceV4 {
        IfaceV4 {
            name: name.into(),
            ip: Ipv4Addr::from(ip),
            netmask: Ipv4Addr::from(mask),
        }
    }

    #[test]
    fn slash24_yields_all_hosts_except_self() {
        let targets = scan_targets(&[iface("en0", [192, 168, 0, 10], [255, 255, 255, 0])]);
        assert_eq!(targets.len(), 253);
        assert!(!targets.contains(&Ipv4Addr::new(192, 168, 0, 10)));
        assert!(targets.contains(&Ipv4Addr::new(192, 168, 0, 1)));
        assert!(targets.contains(&Ipv4Addr::new(192, 168, 0, 254)));
    }

    #[test]
    fn wide_prefix_is_skipped() {
        let targets = scan_targets(&[iface("en0", [10, 0, 0, 5], [255, 0, 0, 0])]);
        assert!(targets.is_empty(), "a /8 subnet must not be swept");
    }

    #[test]
    fn loopback_is_skipped() {
        let targets = scan_targets(&[iface("lo0", [127, 0, 0, 1], [255, 0, 0, 0])]);
        assert!(targets.is_empty());
    }

    #[test]
    fn link_local_is_skipped() {
        let targets = scan_targets(&[iface("en0", [169, 254, 1, 5], [255, 255, 255, 0])]);
        assert!(targets.is_empty());
    }

    #[test]
    fn awdl_interface_is_skipped() {
        let targets = scan_targets(&[iface("awdl0", [192, 168, 5, 2], [255, 255, 255, 0])]);
        assert!(targets.is_empty());
    }

    #[tokio::test]
    async fn probe_host_resolves_a_live_server() {
        use crate::net::identity::PeerIdentity;
        use crate::net::signaling::server::SignalingServer;
        use crate::net::trust::TrustStore;
        use crate::settings::Settings;
        use crate::SessionManager;
        use std::sync::Arc;
        use tokio::sync::RwLock;
        use uuid::Uuid;

        let dir = tempfile::tempdir().unwrap();
        let identity = PeerIdentity {
            peer_id: Uuid::new_v4(),
            peer_name: "Studio PC".into(),
        };
        let trust = Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("trust.toml")).unwrap(),
        ));
        let server = SignalingServer::start(
            "127.0.0.1:0".parse().unwrap(),
            identity.clone(),
            trust,
            SessionManager::new(),
            Arc::new(RwLock::new(Settings::default())),
        )
        .await
        .unwrap();

        let found = probe_host(server.bind_addr, Duration::from_secs(2))
            .await
            .expect("probe must resolve a live server");
        assert_eq!(found.peer_id, identity.peer_id.to_string());
        assert_eq!(found.peer_name, "Studio PC");
        assert_eq!(found.port, server.bind_addr.port());
        assert_eq!(found.version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn probe_host_on_dead_addr_returns_none() {
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let got = probe_host(dead, Duration::from_millis(300)).await;
        assert!(got.is_none(), "probing a closed port must yield None");
    }

    #[test]
    fn multiple_interfaces_dedup_overlapping_hosts() {
        let targets = scan_targets(&[
            iface("en0", [192, 168, 0, 10], [255, 255, 255, 0]),
            iface("en1", [192, 168, 0, 20], [255, 255, 255, 0]),
        ]);
        let unique: std::collections::HashSet<_> = targets.iter().collect();
        assert_eq!(unique.len(), targets.len(), "targets must be deduplicated");
        assert_eq!(targets.len(), 252, "254 hosts minus the two local IPs");
        assert!(!targets.contains(&Ipv4Addr::new(192, 168, 0, 10)));
        assert!(!targets.contains(&Ipv4Addr::new(192, 168, 0, 20)));
    }
}
