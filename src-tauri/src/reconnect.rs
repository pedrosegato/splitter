use crate::core::AppCore;
use splitter_core::net::signaling::connect_to_peer;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub fn spawn_reconnect(core: Arc<AppCore>, peer_id: Uuid, _addr: SocketAddr) {
    tokio::spawn(async move {
        let in_outgoing = core.outgoing.read().await.contains_key(&peer_id);
        let in_mdns = core.peers.read().await.contains_key(&peer_id.to_string());
        if !in_outgoing && !in_mdns {
            tracing::debug!(%peer_id, "skip reconnect: no dialable address for {peer_id}");
            return;
        }

        let delays_secs: [u64; 10] = [1, 2, 4, 8, 16, 30, 30, 30, 30, 30];
        for (attempt, delay) in delays_secs.iter().enumerate() {
            tokio::time::sleep(Duration::from_secs(*delay)).await;

            let mdns_addr = {
                let map = core.peers.read().await;
                map.values()
                    .find(|p| p.peer_id == peer_id.to_string())
                    .map(|p| format!("{}:{}", p.host, p.port))
                    .and_then(|s| s.parse::<SocketAddr>().ok())
            };
            let current_addr = match mdns_addr {
                Some(a) => a,
                None => {
                    let og = core.outgoing.read().await;
                    match og.get(&peer_id).map(|h| h.remote_addr) {
                        Some(a) => a,
                        None => {
                            tracing::debug!(%peer_id, "no dialable address; aborting reconnect");
                            return;
                        }
                    }
                }
            };

            tracing::debug!(%peer_id, attempt, %current_addr, "reconnect attempt");

            let identity = core.identity.read().clone();
            match connect_to_peer(
                current_addr,
                &identity,
                core.trust.clone(),
                Some(peer_id),
                Duration::from_secs(5),
            )
            .await
            {
                Ok(outcome) if outcome.accepted => {
                    tracing::info!(%peer_id, "reconnected to peer");
                    if let Some(pid) = outcome.remote_peer_id {
                        let events = outcome.handle.events.subscribe();
                        let reconnect_addr = outcome.handle.remote_addr;
                        core.outgoing.write().await.insert(pid, outcome.handle);
                        crate::acceptor::spawn_acceptor(core.clone(), pid, events, reconnect_addr);
                    }
                    return;
                }
                Ok(_) => {
                    tracing::debug!(%peer_id, attempt, "reconnect not accepted, retrying");
                }
                Err(e) => {
                    tracing::debug!(%peer_id, attempt, "reconnect failed: {e}");
                }
            }

            let still_present = core.peers.read().await.contains_key(&peer_id.to_string());
            if !still_present {
                tracing::debug!(%peer_id, "peer no longer in mDNS; aborting reconnect");
                return;
            }
        }

        tracing::warn!(%peer_id, "reconnect exhausted all attempts");
    });
}
