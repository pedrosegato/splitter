use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use splitter_core::net::signaling::connect_to_peer;
use crate::core::AppCore;

pub fn spawn_reconnect(core: Arc<AppCore>, peer_id: Uuid, addr: SocketAddr) {
    tokio::spawn(async move {
        let delays_secs: [u64; 10] = [1, 2, 4, 8, 16, 30, 30, 30, 30, 30];
        for (attempt, delay) in delays_secs.iter().enumerate() {
            tokio::time::sleep(Duration::from_secs(*delay)).await;

            let current_addr = {
                let map = core.peers.read().await;
                map.values()
                    .find(|p| p.peer_id == peer_id.to_string())
                    .map(|p| format!("{}:{}", p.host, p.port))
                    .and_then(|s| s.parse::<SocketAddr>().ok())
                    .unwrap_or(addr)
            };

            tracing::debug!(%peer_id, attempt, %current_addr, "reconnect attempt");

            match connect_to_peer(
                current_addr,
                &core.identity,
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

            let still_present = core
                .peers
                .read()
                .await
                .contains_key(&peer_id.to_string());
            if !still_present {
                tracing::debug!(%peer_id, "peer no longer in mDNS; aborting reconnect");
                return;
            }
        }

        tracing::warn!(%peer_id, "reconnect exhausted all attempts");
    });
}
